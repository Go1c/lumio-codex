//! fns-transport: authenticated, reconnecting workspace-sync-v2 WebSocket client.
//!
//! Owns the loopback-only endpoint, sensitive Bearer upgrade, one-reader/one-writer
//! WebSocket session, request correlation, bounded blob transfers, reconnect
//! scheduling, and an engine worker that serializes calls into fns-sync-core.

pub mod config;
pub mod engine;
pub mod error;
pub mod reconnect;
pub mod socket;

pub use config::{
    CONNECT_TIMEOUT, DEFAULT_MAX_ACTIVE_TRANSFERS, ENGINE_QUEUE_CAPACITY, INBOUND_QUEUE_CAPACITY,
    LOCAL_CHANGE_QUEUE_CAPACITY, MAX_ACTIVE_TRANSFERS, MAX_IN_FLIGHT_REQUESTS,
    MAX_RECOVERED_COMMANDS, MAX_REQUEST_IDS_PER_CONNECTION, MAX_TRANSFER_IDS_PER_CONNECTION,
    NOTICE_QUEUE_CAPACITY, OUTBOUND_QUEUE_CAPACITY, REQUEST_TIMEOUT, SHUTDOWN_GRACE,
    TRANSFER_IDLE_TIMEOUT, TRANSFER_MAX_LIFETIME, WORKSPACE_V2_PATH, WorkspaceEndpoint,
    WorkspaceTransportConfig,
};
pub use engine::{EngineHandle, EngineWorker};
pub use error::{TransportError, TransportErrorCode};
pub use reconnect::{JitterSource, ReconnectPolicy, ReconnectSchedule, UuidJitter};
