//! Workspace transport configuration: loopback-only endpoint validation and
//! fixed bounded capacities/timeouts.

use crate::error::{TransportError, TransportErrorCode};

use std::fmt;
use std::path::PathBuf;

use url::Url;

/// The exact server endpoint path for workspace sync v2.
pub const WORKSPACE_V2_PATH: &str = "/api/user/workspace-sync/v2";

// Fixed bounded queue / table capacities (see plan Global Constraints).
pub const INBOUND_QUEUE_CAPACITY: usize = 8;
pub const OUTBOUND_QUEUE_CAPACITY: usize = 8;
pub const ENGINE_QUEUE_CAPACITY: usize = 64;
pub const LOCAL_CHANGE_QUEUE_CAPACITY: usize = 64;
pub const NOTICE_QUEUE_CAPACITY: usize = 32;
pub const MAX_IN_FLIGHT_REQUESTS: usize = 64;
pub const MAX_REQUEST_IDS_PER_CONNECTION: usize = 4_096;
pub const MAX_TRANSFER_IDS_PER_CONNECTION: usize = 1_024;
pub const MAX_RECOVERED_COMMANDS: usize = 100_002;
pub const DEFAULT_MAX_ACTIVE_TRANSFERS: usize = 2;
pub const MAX_ACTIVE_TRANSFERS: usize = 4;

// Fixed timeouts.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const TRANSFER_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const TRANSFER_MAX_LIFETIME: std::time::Duration = std::time::Duration::from_secs(30 * 60);
pub const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// A validated workspace-sync-v2 WebSocket endpoint.
///
/// Rules: scheme `ws`, explicit nonzero port, host exactly `127.0.0.1` or `::1`,
/// path exactly `/api/user/workspace-sync/v2`. UserInfo, query, fragment, other
/// hosts, `wss`, redirects, proxy discovery, and in-frame Authorization are rejected.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkspaceEndpoint {
    url: Url,
}

impl WorkspaceEndpoint {
    /// Parse and validate a workspace endpoint URL.
    pub fn parse(value: &str) -> Result<Self, TransportError> {
        let url = Url::parse(value)
            .map_err(|_| TransportError::new(TransportErrorCode::InvalidConfiguration, false))?;

        // Scheme must be ws (not wss — TLS belongs to Task 7 SSH LocalForward layer).
        if url.scheme() != "ws" {
            return Err(TransportError::new(
                TransportErrorCode::InvalidConfiguration,
                false,
            ));
        }

        // No userinfo, query, or fragment.
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(TransportError::new(
                TransportErrorCode::InvalidConfiguration,
                false,
            ));
        }

        // Exact path.
        if url.path() != WORKSPACE_V2_PATH {
            return Err(TransportError::new(
                TransportErrorCode::InvalidConfiguration,
                false,
            ));
        }

        // Host must be a loopback IP literal (127.0.0.1 or ::1), not localhost/hostname.
        let is_loopback_ip = match url.host() {
            Some(url::Host::Ipv4(addr)) => addr.is_loopback(),
            Some(url::Host::Ipv6(addr)) => addr.is_loopback(),
            _ => false,
        };
        if !is_loopback_ip {
            return Err(TransportError::new(
                TransportErrorCode::InvalidConfiguration,
                false,
            ));
        }

        // Explicit nonzero port.
        if url.port().is_none() {
            return Err(TransportError::new(
                TransportErrorCode::InvalidConfiguration,
                false,
            ));
        }

        Ok(Self { url })
    }

    /// Return the validated URL.
    pub fn as_url(&self) -> &Url {
        &self.url
    }
}

impl fmt::Debug for WorkspaceEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Only scheme, loopback IP, port, and path — nothing sensitive.
        f.debug_struct("WorkspaceEndpoint")
            .field("scheme", &self.url.scheme())
            .field("host", &self.url.host_str())
            .field("port", &self.url.port())
            .field("path", &self.url.path())
            .finish()
    }
}

/// Transport configuration for one workspace connection.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkspaceTransportConfig {
    pub endpoint: WorkspaceEndpoint,
    pub workspace_id: fns_protocol::WorkspaceId,
    pub client_id: fns_protocol::ClientId,
    pub state_dir: PathBuf,
    pub max_active_transfers: usize,
}

impl fmt::Debug for WorkspaceTransportConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Omit state_dir; print only loopback endpoint, IDs, and transfer limit.
        f.debug_struct("WorkspaceTransportConfig")
            .field("endpoint", &self.endpoint)
            .field("workspace_id", &self.workspace_id)
            .field("client_id", &self.client_id)
            .field("max_active_transfers", &self.max_active_transfers)
            .finish()
    }
}
