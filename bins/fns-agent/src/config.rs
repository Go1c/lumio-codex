//! Agent configuration: strict JSON parsing and validation.

use crate::error::{AgentError, AgentErrorCode};

use std::path::{Path, PathBuf};

/// Agent configuration loaded from a strict JSON file.
/// Token is never in this struct — it lives in a separate token file.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConfig {
    pub schema_version: String,
    pub endpoint: String,
    pub workspace_id: fns_protocol::WorkspaceId,
    pub client_id: fns_protocol::ClientId,
    pub workspace_root: PathBuf,
    pub state_dir: PathBuf,
    pub token_file: PathBuf,
    pub sync: AgentSyncConfig,
    pub transport: AgentTransportConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSyncConfig {
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub protect_secrets: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTransportConfig {
    pub max_active_transfers: usize,
}

impl AgentConfig {
    /// Load and validate configuration from a JSON file.
    /// On Linux, verifies the config file and token file are private regular files.
    pub fn load_linux(path: &Path) -> Result<Self, AgentError> {
        // Verify config file is private on Linux.
        #[cfg(target_os = "linux")]
        {
            fns_platform::verify_private_regular_linux(path)
                .map_err(|_| AgentError::new(AgentErrorCode::InsecureCredential))?;
        }

        // Read and bound the file.
        let bytes = std::fs::read(path)
            .map_err(|_| AgentError::new(AgentErrorCode::InvalidConfiguration))?;
        if bytes.len() > 65_536 {
            return Err(AgentError::new(AgentErrorCode::InvalidConfiguration));
        }

        // Strict decode.
        let config: AgentConfig = serde_json::from_slice(&bytes)
            .map_err(|_| AgentError::new(AgentErrorCode::InvalidConfiguration))?;

        // Validate schema version.
        if config.schema_version != "fns-agent-config/1" {
            return Err(AgentError::new(AgentErrorCode::InvalidConfiguration));
        }

        // Validate paths are absolute.
        if !config.workspace_root.is_absolute()
            || !config.state_dir.is_absolute()
            || !config.token_file.is_absolute()
        {
            return Err(AgentError::new(AgentErrorCode::InvalidConfiguration));
        }

        // Validate endpoint is loopback-only.
        fns_transport::WorkspaceEndpoint::parse(&config.endpoint)
            .map_err(|_| AgentError::new(AgentErrorCode::InvalidConfiguration))?;

        // Validate transfer limit.
        if config.transport.max_active_transfers == 0
            || config.transport.max_active_transfers > fns_transport::MAX_ACTIVE_TRANSFERS
        {
            return Err(AgentError::new(AgentErrorCode::InvalidConfiguration));
        }

        Ok(config)
    }

    /// Default config path following XDG conventions.
    pub fn default_config_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg).join("fns-workspace").join("agent.json")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".config")
                .join("fns-workspace")
                .join("agent.json")
        } else {
            PathBuf::from(".config/fns-workspace/agent.json")
        }
    }
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("schema_version", &self.schema_version)
            .field("workspace_id", &self.workspace_id)
            .field("client_id", &self.client_id)
            .field("max_active_transfers", &self.transport.max_active_transfers)
            .finish()
    }
}
