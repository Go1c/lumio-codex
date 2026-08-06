use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{
    ProtocolDecodeError, ProtocolEncodeError, WorkspaceValidationError,
    error::validation_error,
    message::{
        WorkspaceAckRequest, WorkspaceBlobBeginMessage, WorkspaceBlobEndMessage,
        WorkspaceBlobNeedDownloadRequest, WorkspaceBlobNeedDownloadResponse,
        WorkspaceBlobNeedUploadPush, WorkspaceConflictCreatedMessage,
        WorkspaceConflictResolvedMessage, WorkspaceConflictResolvedRequest, WorkspaceEventMessage,
        WorkspaceHelloRequest, WorkspaceHelloResponse, WorkspaceMutation,
        WorkspaceMutationAcceptedMessage, WorkspaceMutationRejectedMessage,
        WorkspaceSnapshotBeginMessage, WorkspaceSnapshotEndMessage, WorkspaceSnapshotEntryMessage,
        WorkspaceSubscribeRequest,
    },
    strict_json,
};

macro_rules! define_workspace_flows {
    ($(($variant:ident, $wire:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum WorkspaceFlow {
            $($variant),+
        }

        impl WorkspaceFlow {
            pub const ALL: [Self; define_workspace_flows!(@count $($variant),+)] = [
                $(Self::$variant),+
            ];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }

        impl fmt::Display for WorkspaceFlow {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for WorkspaceFlow {
            type Err = WorkspaceValidationError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($wire => Ok(Self::$variant)),+,
                    _ => Err(validation_error("flow", "unknown_flow")),
                }
            }
        }

        impl Serialize for WorkspaceFlow {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for WorkspaceFlow {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::from_str(&value).map_err(D::Error::custom)
            }
        }
    };
    (@count $($variant:ident),+) => {
        <[()]>::len(&[$(define_workspace_flows!(@unit $variant)),+])
    };
    (@unit $variant:ident) => { () };
}

define_workspace_flows!(
    (ClientRequest, "client_request"),
    (ServerResponse, "server_response"),
    (ServerPush, "server_push"),
);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActionFlowSpec {
    pub action: WorkspaceAction,
    pub flow: WorkspaceFlow,
    pub body_kind: MessageBodyKind,
}

macro_rules! define_workspace_protocol {
    (
        $(
            $action_variant:ident => $token:literal {
                $(
                    $body_variant:ident($body_type:ty) => [$($flow:ident),+ $(,)?]
                ),+ $(,)?
            }
        ),+ $(,)?
    ) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum WorkspaceAction {
            $($action_variant),+
        }

        impl WorkspaceAction {
            pub const ALL: [Self; define_workspace_protocol!(@count $($action_variant),+)] = [
                $(Self::$action_variant),+
            ];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$action_variant => $token),+
                }
            }
        }

        impl fmt::Display for WorkspaceAction {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for WorkspaceAction {
            type Err = WorkspaceValidationError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($token => Ok(Self::$action_variant)),+,
                    _ => Err(validation_error("action", "unknown_action")),
                }
            }
        }

        impl Serialize for WorkspaceAction {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for WorkspaceAction {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::from_str(&value).map_err(D::Error::custom)
            }
        }

        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum MessageBodyKind {
            $($($body_variant),+),+
        }

        #[derive(Clone, Debug, Eq, PartialEq)]
        pub enum MessageBody {
            $($($body_variant($body_type)),+),+
        }

        impl MessageBody {
            pub const fn kind(&self) -> MessageBodyKind {
                match self {
                    $($(Self::$body_variant(_) => MessageBodyKind::$body_variant),+),+
                }
            }

            pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
                match self {
                    $($(Self::$body_variant(body) => body.validate()),+),+
                }
            }
        }

        impl Serialize for MessageBody {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                match self {
                    $($(Self::$body_variant(body) => body.serialize(serializer)),+),+
                }
            }
        }

        pub const ACTION_FLOW_SPECS: &[ActionFlowSpec] = &[
            $($($(
                ActionFlowSpec {
                    action: WorkspaceAction::$action_variant,
                    flow: WorkspaceFlow::$flow,
                    body_kind: MessageBodyKind::$body_variant,
                },
            )+)+)+
        ];

        pub fn decode_data(
            action: WorkspaceAction,
            flow: WorkspaceFlow,
            data: &[u8],
        ) -> Result<MessageBody, ProtocolDecodeError> {
            if !ACTION_FLOW_SPECS
                .iter()
                .any(|spec| spec.action == action && spec.flow == flow)
            {
                return Err(validation_error("flow", "flow_not_allowed").into());
            }
            match (action, flow) {
                $($($(
                    (WorkspaceAction::$action_variant, WorkspaceFlow::$flow) => {
                        strict_json::from_slice::<$body_type>(data)
                            .map(MessageBody::$body_variant)
                            .map_err(|_| validation_error("data", "invalid_json").into())
                    }
                )+)+)+
                _ => unreachable!("registry precheck and generated decoder must agree"),
            }
        }
    };
    (@count $($variant:ident),+) => {
        <[()]>::len(&[$(define_workspace_protocol!(@unit $variant)),+])
    };
    (@unit $variant:ident) => { () };
}

define_workspace_protocol!(
    WorkspaceHello => "WorkspaceHello" {
        HelloRequest(WorkspaceHelloRequest) => [ClientRequest],
        HelloResponse(WorkspaceHelloResponse) => [ServerResponse],
    },
    WorkspaceSubscribe => "WorkspaceSubscribe" {
        SubscribeRequest(WorkspaceSubscribeRequest) => [ClientRequest],
    },
    WorkspaceSnapshotBegin => "WorkspaceSnapshotBegin" {
        SnapshotBegin(WorkspaceSnapshotBeginMessage) => [ServerPush],
    },
    WorkspaceSnapshotEntry => "WorkspaceSnapshotEntry" {
        SnapshotEntry(WorkspaceSnapshotEntryMessage) => [ServerPush],
    },
    WorkspaceSnapshotEnd => "WorkspaceSnapshotEnd" {
        SnapshotEnd(WorkspaceSnapshotEndMessage) => [ServerPush],
    },
    WorkspaceMutation => "WorkspaceMutation" {
        Mutation(WorkspaceMutation) => [ClientRequest],
    },
    WorkspaceMutationAccepted => "WorkspaceMutationAccepted" {
        MutationAccepted(WorkspaceMutationAcceptedMessage) => [ServerResponse],
    },
    WorkspaceMutationRejected => "WorkspaceMutationRejected" {
        MutationRejected(WorkspaceMutationRejectedMessage) => [ServerResponse],
    },
    WorkspaceEvent => "WorkspaceEvent" {
        Event(WorkspaceEventMessage) => [ServerPush],
    },
    WorkspaceAck => "WorkspaceAck" {
        Ack(WorkspaceAckRequest) => [ClientRequest, ServerResponse],
    },
    WorkspaceBlobNeed => "WorkspaceBlobNeed" {
        BlobNeedDownloadRequest(WorkspaceBlobNeedDownloadRequest) => [ClientRequest],
        BlobNeedDownloadResponse(WorkspaceBlobNeedDownloadResponse) => [ServerResponse],
        BlobNeedUploadPush(WorkspaceBlobNeedUploadPush) => [ServerPush],
    },
    WorkspaceBlobBegin => "WorkspaceBlobBegin" {
        BlobBegin(WorkspaceBlobBeginMessage) => [ClientRequest, ServerResponse, ServerPush],
    },
    WorkspaceBlobEnd => "WorkspaceBlobEnd" {
        BlobEnd(WorkspaceBlobEndMessage) => [ClientRequest, ServerResponse, ServerPush],
    },
    WorkspaceConflictCreated => "WorkspaceConflictCreated" {
        ConflictCreated(WorkspaceConflictCreatedMessage) => [ServerPush],
    },
    WorkspaceConflictResolved => "WorkspaceConflictResolved" {
        ConflictResolvedRequest(WorkspaceConflictResolvedRequest) => [ClientRequest],
        ConflictResolved(WorkspaceConflictResolvedMessage) => [ServerResponse, ServerPush],
    },
);

pub(crate) fn valid_action_token(action: &str) -> bool {
    let bytes = action.as_bytes();
    bytes.len() <= crate::MAX_ACTION_BYTES
        && bytes
            .first()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && bytes[1..].iter().all(|byte| byte.is_ascii_alphanumeric())
}

pub(crate) fn body_allowed(
    action: WorkspaceAction,
    flow: WorkspaceFlow,
    body: &MessageBody,
) -> bool {
    ACTION_FLOW_SPECS
        .iter()
        .any(|spec| spec.action == action && spec.flow == flow && spec.body_kind == body.kind())
}

pub(crate) fn encode_body(body: &MessageBody) -> Result<Vec<u8>, ProtocolEncodeError> {
    serde_json::to_vec(body).map_err(|_| validation_error("data", "serialization_failed").into())
}
