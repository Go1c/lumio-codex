use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use crate::{
    MAX_CONTROL_FRAME_BYTES, MessageBody, ProtocolDecodeError, ProtocolEncodeError, RequestId,
    WorkspaceAction, WorkspaceFlow, WorkspaceV2Error, WorkspaceV2ErrorCode,
    action::{ACTION_FLOW_SPECS, body_allowed, encode_body, valid_action_token},
    error::validation_error,
    strict_json,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedFrame {
    pub action: WorkspaceAction,
    pub flow: WorkspaceFlow,
    pub envelope: DecodedEnvelope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodedEnvelope {
    Request {
        request_id: RequestId,
        body: MessageBody,
    },
    Success {
        request_id: Option<RequestId>,
        body: MessageBody,
    },
    Failure {
        request_id: Option<RequestId>,
        error: WorkspaceV2Error,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum Presence<T> {
    #[default]
    Missing,
    Value(T),
}

impl<T> Presence<T> {
    const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    fn into_option(self) -> Option<T> {
        match self {
            Self::Missing => None,
            Self::Value(value) => Some(value),
        }
    }
}

impl<T> Serialize for Presence<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T> Deserialize<'de> for Presence<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Value)
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireEnvelope {
    #[serde(default, skip_serializing_if = "Presence::is_missing")]
    request_id: Presence<RequestId>,
    #[serde(default, skip_serializing_if = "Presence::is_missing")]
    status: Presence<bool>,
    #[serde(default, skip_serializing_if = "Presence::is_missing")]
    data: Presence<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Presence::is_missing")]
    error: Presence<WorkspaceV2Error>,
}

pub fn decode_text_frame(
    frame: &[u8],
    flow: WorkspaceFlow,
) -> Result<DecodedFrame, ProtocolDecodeError> {
    let (action, envelope) = parse_text_frame(frame)?;
    decode_parsed_frame(action, flow, envelope)
}

pub fn decode_server_text_frame(frame: &[u8]) -> Result<DecodedFrame, ProtocolDecodeError> {
    let (action, envelope) = parse_text_frame(frame)?;
    let flow = match (&envelope.status, &envelope.request_id) {
        (Presence::Value(false), _) => WorkspaceFlow::ServerResponse,
        (Presence::Value(true), Presence::Value(_)) => WorkspaceFlow::ServerResponse,
        (Presence::Value(true), Presence::Missing) => WorkspaceFlow::ServerPush,
        (Presence::Missing, _) => return Err(validation_error("status", "required").into()),
    };
    decode_parsed_frame(action, flow, envelope)
}

fn parse_text_frame(frame: &[u8]) -> Result<(WorkspaceAction, WireEnvelope), ProtocolDecodeError> {
    if frame.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(validation_error("frame", "too_large").into());
    }
    let separator = frame
        .iter()
        .position(|byte| *byte == b'|')
        .ok_or_else(|| ProtocolDecodeError::from(validation_error("frame", "missing_separator")))?;
    let action_bytes = &frame[..separator];
    let action_text = std::str::from_utf8(action_bytes)
        .map_err(|_| ProtocolDecodeError::from(validation_error("action", "invalid_utf8")))?;
    if !valid_action_token(action_text) {
        return Err(validation_error("action", "invalid_token").into());
    }
    let action = WorkspaceAction::from_str(action_text).map_err(ProtocolDecodeError::from)?;
    let envelope = strict_json::from_slice::<WireEnvelope>(&frame[separator + 1..])
        .map_err(|_| ProtocolDecodeError::from(validation_error("envelope", "invalid_json")))?;
    Ok((action, envelope))
}

fn decode_parsed_frame(
    action: WorkspaceAction,
    flow: WorkspaceFlow,
    envelope: WireEnvelope,
) -> Result<DecodedFrame, ProtocolDecodeError> {
    let decoded = match flow {
        WorkspaceFlow::ClientRequest => decode_request(action, envelope)?,
        WorkspaceFlow::ServerResponse => decode_response(action, envelope)?,
        WorkspaceFlow::ServerPush => decode_push(action, envelope)?,
    };
    Ok(DecodedFrame {
        action,
        flow,
        envelope: decoded,
    })
}

pub fn encode_request(
    action: WorkspaceAction,
    request_id: RequestId,
    body: MessageBody,
) -> Result<Vec<u8>, ProtocolEncodeError> {
    check_body(action, WorkspaceFlow::ClientRequest, &body)?;
    let data = raw_body(&body)?;
    encode_wire(
        action.as_str(),
        &WireEnvelope {
            request_id: Presence::Value(request_id),
            status: Presence::Missing,
            data: Presence::Value(data),
            error: Presence::Missing,
        },
    )
}

pub fn encode_success(
    action: WorkspaceAction,
    flow: WorkspaceFlow,
    request_id: Option<RequestId>,
    body: MessageBody,
) -> Result<Vec<u8>, ProtocolEncodeError> {
    match flow {
        WorkspaceFlow::ClientRequest => {
            return Err(validation_error("flow", "flow_not_allowed").into());
        }
        WorkspaceFlow::ServerResponse if request_id.is_none() => {
            return Err(validation_error("requestId", "required_for_response").into());
        }
        WorkspaceFlow::ServerPush if request_id.is_some() => {
            return Err(validation_error("requestId", "forbidden_for_push").into());
        }
        WorkspaceFlow::ServerResponse | WorkspaceFlow::ServerPush => {}
    }
    check_body(action, flow, &body)?;
    let data = raw_body(&body)?;
    encode_wire(
        action.as_str(),
        &WireEnvelope {
            request_id: request_id.map_or(Presence::Missing, Presence::Value),
            status: Presence::Value(true),
            data: Presence::Value(data),
            error: Presence::Missing,
        },
    )
}

pub fn encode_failure(
    action: WorkspaceAction,
    request_id: Option<RequestId>,
    error: WorkspaceV2Error,
) -> Result<Vec<u8>, ProtocolEncodeError> {
    error.validate().map_err(ProtocolEncodeError::from)?;
    encode_wire(
        action.as_str(),
        &WireEnvelope {
            request_id: request_id.map_or(Presence::Missing, Presence::Value),
            status: Presence::Value(false),
            data: Presence::Missing,
            error: Presence::Value(error),
        },
    )
}

pub fn encode_unknown_action_failure(
    received_action: &str,
    request_id: Option<RequestId>,
) -> Result<Vec<u8>, ProtocolEncodeError> {
    if !valid_action_token(received_action) {
        return Err(validation_error("action", "invalid_token").into());
    }
    if WorkspaceAction::from_str(received_action).is_ok() {
        return Err(validation_error("action", "registered_action").into());
    }
    let error = WorkspaceV2Error::new(WorkspaceV2ErrorCode::UnknownAction, vec![]);
    encode_wire(
        received_action,
        &WireEnvelope {
            request_id: request_id.map_or(Presence::Missing, Presence::Value),
            status: Presence::Value(false),
            data: Presence::Missing,
            error: Presence::Value(error),
        },
    )
}

fn decode_request(
    action: WorkspaceAction,
    envelope: WireEnvelope,
) -> Result<DecodedEnvelope, ProtocolDecodeError> {
    let request_id = match envelope.request_id {
        Presence::Value(request_id) => request_id,
        Presence::Missing => return Err(validation_error("requestId", "required").into()),
    };
    if !envelope.status.is_missing() {
        return Err(validation_error("status", "forbidden_for_request").into());
    }
    if !envelope.error.is_missing() {
        return Err(validation_error("error", "forbidden_for_request").into());
    }
    let data = match envelope.data {
        Presence::Value(data) => data,
        Presence::Missing => return Err(validation_error("data", "required").into()),
    };
    let body = crate::decode_data(action, WorkspaceFlow::ClientRequest, data.get().as_bytes())?;
    Ok(DecodedEnvelope::Request { request_id, body })
}

fn decode_response(
    action: WorkspaceAction,
    envelope: WireEnvelope,
) -> Result<DecodedEnvelope, ProtocolDecodeError> {
    let status = match envelope.status {
        Presence::Value(status) => status,
        Presence::Missing => return Err(validation_error("status", "required").into()),
    };
    if status {
        let request_id = match envelope.request_id {
            Presence::Value(request_id) => request_id,
            Presence::Missing => {
                return Err(validation_error("requestId", "required_for_response").into());
            }
        };
        if !envelope.error.is_missing() {
            return Err(validation_error("error", "success_forbids_error").into());
        }
        let data = match envelope.data {
            Presence::Value(data) => data,
            Presence::Missing => {
                return Err(validation_error("data", "success_requires_data").into());
            }
        };
        let body =
            crate::decode_data(action, WorkspaceFlow::ServerResponse, data.get().as_bytes())?;
        Ok(DecodedEnvelope::Success {
            request_id: Some(request_id),
            body,
        })
    } else {
        if !envelope.data.is_missing() {
            return Err(validation_error("data", "error_forbids_data").into());
        }
        let error = match envelope.error {
            Presence::Value(error) => error,
            Presence::Missing => {
                return Err(validation_error("error", "error_requires_error").into());
            }
        };
        error.validate().map_err(ProtocolDecodeError::from)?;
        Ok(DecodedEnvelope::Failure {
            request_id: envelope.request_id.into_option(),
            error,
        })
    }
}

fn decode_push(
    action: WorkspaceAction,
    envelope: WireEnvelope,
) -> Result<DecodedEnvelope, ProtocolDecodeError> {
    if !envelope.request_id.is_missing() {
        return Err(validation_error("requestId", "forbidden_for_push").into());
    }
    match envelope.status {
        Presence::Value(true) => {}
        Presence::Value(false) => {
            return Err(validation_error("status", "push_requires_success").into());
        }
        Presence::Missing => return Err(validation_error("status", "required").into()),
    }
    if !envelope.error.is_missing() {
        return Err(validation_error("error", "success_forbids_error").into());
    }
    let data = match envelope.data {
        Presence::Value(data) => data,
        Presence::Missing => return Err(validation_error("data", "success_requires_data").into()),
    };
    let body = crate::decode_data(action, WorkspaceFlow::ServerPush, data.get().as_bytes())?;
    Ok(DecodedEnvelope::Success {
        request_id: None,
        body,
    })
}

fn check_body(
    action: WorkspaceAction,
    flow: WorkspaceFlow,
    body: &MessageBody,
) -> Result<(), ProtocolEncodeError> {
    if !ACTION_FLOW_SPECS
        .iter()
        .any(|spec| spec.action == action && spec.flow == flow)
    {
        return Err(validation_error("flow", "flow_not_allowed").into());
    }
    if !body_allowed(action, flow, body) {
        return Err(validation_error("data", "type_mismatch").into());
    }
    Ok(())
}

fn raw_body(body: &MessageBody) -> Result<Box<RawValue>, ProtocolEncodeError> {
    let encoded = encode_body(body)?;
    let encoded = String::from_utf8(encoded)
        .map_err(|_| ProtocolEncodeError::from(validation_error("data", "serialization_failed")))?;
    RawValue::from_string(encoded)
        .map_err(|_| ProtocolEncodeError::from(validation_error("data", "serialization_failed")))
}

fn encode_wire(action: &str, envelope: &WireEnvelope) -> Result<Vec<u8>, ProtocolEncodeError> {
    let payload = serde_json::to_vec(envelope).map_err(|_| {
        ProtocolEncodeError::from(validation_error("envelope", "serialization_failed"))
    })?;
    let mut frame = Vec::with_capacity(action.len() + 1 + payload.len());
    frame.extend_from_slice(action.as_bytes());
    frame.push(b'|');
    frame.extend_from_slice(&payload);
    if frame.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(validation_error("frame", "too_large").into());
    }
    Ok(frame)
}
