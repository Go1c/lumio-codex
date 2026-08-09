//! Project configuration persistence.
//!
//! Non-sensitive config is stored as JSON in the app's config directory.
//! Tokens are never stored here — they go to macOS Keychain (Task 7 later step).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectClientIdentity(fns_protocol::ClientId);

impl ProjectClientIdentity {
    pub fn load_or_create_in(
        root: &std::path::Path,
        project_id: &str,
    ) -> Result<Self, std::io::Error> {
        std::fs::create_dir_all(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = root.join(format!("client-{project_id}.json"));
        match std::fs::read(&path) {
            Ok(bytes) => return decode_client_identity(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let client_id = fns_protocol::ClientId::parse(&uuid::Uuid::new_v4().to_string())
            .map_err(|_| std::io::Error::other("client identity generation failed"))?;
        let payload = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": "fns-client-identity/1",
            "clientId": client_id,
        }))
        .map_err(std::io::Error::other)?;
        let temporary = root.join(format!(".client-{project_id}-{}.tmp", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        use std::io::Write;
        if let Err(error) = file.write_all(&payload).and_then(|()| file.sync_all()) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        drop(file);
        match std::fs::hard_link(&temporary, &path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&temporary);
                #[cfg(unix)]
                std::fs::File::open(root)?.sync_all()?;
                Ok(Self(client_id))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = std::fs::remove_file(&temporary);
                decode_client_identity(&std::fs::read(path)?)
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                Err(error)
            }
        }
    }

    pub fn get(self) -> fns_protocol::ClientId {
        self.0
    }
}

fn decode_client_identity(bytes: &[u8]) -> Result<ProjectClientIdentity, std::io::Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct StoredIdentity {
        schema_version: String,
        client_id: fns_protocol::ClientId,
    }
    let identity: StoredIdentity = serde_json::from_slice(bytes).map_err(std::io::Error::other)?;
    if identity.schema_version != "fns-client-identity/1" {
        return Err(std::io::Error::other("unsupported client identity"));
    }
    Ok(ProjectClientIdentity(identity.client_id))
}

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

#[cfg(test)]
mod task_2b_identity_tests {
    use super::ProjectClientIdentity;

    #[test]
    fn two_projects_get_distinct_stable_client_ids_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let first = ProjectClientIdentity::load_or_create_in(dir.path(), "project-a").unwrap();
        let second = ProjectClientIdentity::load_or_create_in(dir.path(), "project-b").unwrap();
        assert_ne!(first, second);
        assert_eq!(
            first,
            ProjectClientIdentity::load_or_create_in(dir.path(), "project-a").unwrap()
        );
        assert_eq!(
            second,
            ProjectClientIdentity::load_or_create_in(dir.path(), "project-b").unwrap()
        );
    }
}
