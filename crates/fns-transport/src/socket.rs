//! WebSocket socket boundary: sensitive Bearer upgrade, tungstenite limits,
//! and one-reader/one-writer stream split.

use crate::config::WorkspaceEndpoint;
use crate::error::{TransportError, TransportErrorCode};
use fns_platform::SecretToken;

use futures_util::{SinkExt, StreamExt};
use http::Request;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Generate a random Sec-WebSocket-Key (16 random bytes, base64 encoded).
fn generate_ws_key() -> String {
    let uuid = uuid::Uuid::new_v4();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(uuid.as_bytes());
    // Add more entropy with a second UUID.
    let uuid2 = uuid::Uuid::new_v4();
    let mut key_bytes = [0u8; 16];
    key_bytes.copy_from_slice(uuid2.as_bytes());
    // XOR for a 16-byte key.
    for i in 0..16 {
        bytes[i] ^= key_bytes[i];
    }
    base64_simple_encode(&bytes)
}

/// Simple base64 encoding without external dependency (RFC 4648 standard alphabet).
fn base64_simple_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Build the HTTP upgrade request with a sensitive Bearer Authorization header
/// and fns-agent client metadata headers. The token is zeroized immediately after
/// the header value is constructed.
fn build_upgrade_request(
    endpoint: &WorkspaceEndpoint,
    token: &SecretToken,
    pkg_version: &str,
) -> Result<Request<()>, TransportError> {
    let url = endpoint.as_url();
    let host = url.host_str().unwrap_or("127.0.0.1");
    let port = url.port().unwrap_or(80);

    let mut builder = Request::builder()
        .method("GET")
        .uri(url.as_str())
        .header("Host", format!("{host}:{port}"))
        .header("Upgrade", "websocket")
        .header("Connection", "upgrade")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_ws_key())
        .header("X-Client", "fns-agent")
        .header("X-Client-Name", "fns-agent")
        .header("X-Client-Version", pkg_version)
        .header("User-Agent", format!("fns-agent/{pkg_version}"));

    // Build sensitive Authorization header inside the token exposure scope.
    let auth_header = token.with_exposed(|bytes| {
        let mut bearer = Vec::with_capacity(bytes.len() + 7);
        bearer.extend_from_slice(b"Bearer ");
        bearer.extend_from_slice(bytes);
        let value = http::HeaderValue::from_bytes(&bearer)
            .map_err(|_| TransportError::new(TransportErrorCode::InvalidConfiguration, false))?;
        use zeroize::Zeroize;
        bearer.zeroize();
        Ok::<_, TransportError>(value)
    })?;

    let mut auth_header = auth_header;
    auth_header.set_sensitive(true);
    builder = builder.header("Authorization", auth_header);

    builder
        .body(())
        .map_err(|_| TransportError::new(TransportErrorCode::InvalidConfiguration, false))
}

/// Tungstenite WebSocket configuration with limits derived from the protocol.
fn ws_config() -> WebSocketConfig {
    let max_frame = fns_protocol::BLOB_HEADER_LEN + fns_protocol::BLOB_CHUNK_BYTES as usize;
    WebSocketConfig::default()
        .read_buffer_size(128 * 1024)
        .write_buffer_size(128 * 1024)
        .max_write_buffer_size(2 * max_frame)
        .max_message_size(Some(max_frame))
        .max_frame_size(Some(max_frame))
}

/// Connect to the workspace endpoint with a sensitive Bearer upgrade and return
/// the WebSocket stream. HTTP 401/403 before upgrade are fatal (non-retryable).
pub async fn connect(
    endpoint: &WorkspaceEndpoint,
    token: &SecretToken,
    pkg_version: &str,
) -> Result<WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, TransportError>
{
    let request = build_upgrade_request(endpoint, token, pkg_version)?;

    let connect_fut =
        tokio_tungstenite::connect_async_with_config(request, Some(ws_config()), true);
    let result = tokio::time::timeout(CONNECT_TIMEOUT, connect_fut).await;

    match result {
        Ok(Ok((stream, _response))) => Ok(stream),
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(resp))) => {
            let status = resp.status();
            match status.as_u16() {
                401 => Err(TransportError::new(
                    TransportErrorCode::AuthenticationRejected,
                    false,
                )),
                403 => Err(TransportError::new(TransportErrorCode::Forbidden, false)),
                _ => Err(TransportError::new(TransportErrorCode::Network, true)),
            }
        }
        Ok(Err(_)) => Err(TransportError::new(TransportErrorCode::Network, true)),
        Err(_) => Err(TransportError::new(TransportErrorCode::Network, true)),
    }
}

/// A WebSocket writer handle.
pub struct SocketWriter {
    inner: futures_util::stream::SplitSink<
        WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        tokio_tungstenite::tungstenite::Message,
    >,
}

/// A WebSocket reader handle.
pub struct SocketReader {
    inner: futures_util::stream::SplitStream<
        WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    >,
}

/// Split a WebSocket stream into separate reader and writer handles.
pub fn split(
    stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) -> (SocketWriter, SocketReader) {
    let (sink, stream) = stream.split();
    (SocketWriter { inner: sink }, SocketReader { inner: stream })
}

impl SocketWriter {
    pub async fn send_text(&mut self, data: Vec<u8>) -> Result<(), TransportError> {
        let text = String::from_utf8(data)
            .map_err(|_| TransportError::new(TransportErrorCode::Protocol, false))?;
        let msg = tokio_tungstenite::tungstenite::Message::Text(text.into());
        self.inner
            .send(msg)
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Network, true))
    }

    pub async fn send_binary(&mut self, data: Vec<u8>) -> Result<(), TransportError> {
        let msg = tokio_tungstenite::tungstenite::Message::Binary(data.into());
        self.inner
            .send(msg)
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Network, true))
    }

    pub async fn send_pong(&mut self, data: Vec<u8>) -> Result<(), TransportError> {
        let msg = tokio_tungstenite::tungstenite::Message::Pong(data.into());
        self.inner
            .send(msg)
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Network, true))
    }

    pub async fn close(&mut self) -> Result<(), TransportError> {
        self.inner
            .close()
            .await
            .map_err(|_| TransportError::new(TransportErrorCode::Network, true))
    }
}

/// An inbound WebSocket message with the opcode preserved.
#[derive(Debug)]
pub enum InboundMessage {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

impl SocketReader {
    pub async fn next(&mut self) -> Option<Result<InboundMessage, TransportError>> {
        match self.inner.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(s))) => {
                Some(Ok(InboundMessage::Text(s.as_bytes().to_vec())))
            }
            Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b))) => {
                Some(Ok(InboundMessage::Binary(b.to_vec())))
            }
            Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(b))) => {
                Some(Ok(InboundMessage::Pong(b.to_vec())))
            }
            Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(b))) => {
                Some(Ok(InboundMessage::Ping(b.to_vec())))
            }
            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
                Some(Ok(InboundMessage::Close))
            }
            Some(Ok(_)) => Some(Ok(InboundMessage::Close)),
            Some(Err(_)) => Some(Err(TransportError::new(TransportErrorCode::Network, true))),
            None => None,
        }
    }
}
