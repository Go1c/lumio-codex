//! Project configuration persistence.
//!
//! Non-sensitive config is stored as JSON in the app's config directory.
//! Tokens are never stored here — they go to macOS Keychain (Task 7 later step).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Sync mode — MVP supports only two-way safe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    TwoWaySafe,
}

/// Sync configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfig {
    pub mode: SyncMode,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub protect_secrets: bool,
}

/// Project configuration persisted on the Mac.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub id: uuid::Uuid,
    pub name: String,
    pub ssh_host_alias: String,
    pub remote_root: String,
    pub local_root: String,
    pub workspace_id: uuid::Uuid,
    pub tmux_session: String,
    pub sync: SyncConfig,
}

impl ProjectConfig {
    /// Get the config directory for storing projects.
    fn config_dir() -> Result<PathBuf, std::io::Error> {
        let dir = if cfg!(target_os = "macos") {
            directories::BaseDirs::new()
                .ok_or_else(|| std::io::Error::other("no config dir"))?
                .config_dir()
                .join("fns-workspace")
        } else {
            PathBuf::from(".config/fns-workspace")
        };
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Path to the projects JSON file.
    fn projects_file() -> Result<PathBuf, std::io::Error> {
        Ok(Self::config_dir()?.join("projects.json"))
    }

    /// Save this project to the default location.
    pub fn save_to_default(&self) -> Result<(), std::io::Error> {
        let mut projects = Self::load_raw()?;
        projects.insert(self.id.to_string(), self.clone());
        Self::save_raw(&projects)
    }

    /// List all saved projects.
    pub fn list_all() -> Result<Vec<ProjectConfig>, std::io::Error> {
        let projects = Self::load_raw()?;
        Ok(projects.into_values().collect())
    }

    /// Delete a project by ID.
    pub fn delete(id: &str) -> Result<(), std::io::Error> {
        let mut projects = Self::load_raw()?;
        projects.remove(id);
        Self::save_raw(&projects)
    }

    fn load_raw() -> Result<HashMap<String, ProjectConfig>, std::io::Error> {
        let path = Self::projects_file()?;
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save_raw(projects: &HashMap<String, ProjectConfig>) -> Result<(), std::io::Error> {
        let path = Self::projects_file()?;
        let bytes = serde_json::to_vec_pretty(projects).map_err(std::io::Error::other)?;
        std::fs::write(&path, bytes)
    }
}
