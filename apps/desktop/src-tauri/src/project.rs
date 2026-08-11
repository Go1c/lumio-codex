//! Project configuration persistence.
//!
//! Non-sensitive config is stored as JSON in the app's config directory.
//! Tokens are never stored here — they go to macOS Keychain (Task 7 later step).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

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

/// Roots that must never be used as a project local/remote workspace root.
const DANGEROUS_ROOTS: &[&str] = &[
    "/", "/etc", "/var", "/usr", "/bin", "/sbin", "/System", "/private", "C:\\", "C:/",
];

/// Reject empty, `..`-escaping, and known dangerous system roots.
pub fn validate_workspace_root(root: &str) -> Result<(), String> {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        return Err("workspace root must not be empty".into());
    }
    if trimmed.contains('\0') {
        return Err("workspace root must not contain NUL".into());
    }

    let path = Path::new(trimmed);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("workspace root must not contain '..' components".into());
    }

    // Normalize trailing slashes for exact dangerous-root matching (except bare "/").
    let normalized = if trimmed.len() > 1 && (trimmed.ends_with('/') || trimmed.ends_with('\\')) {
        trimmed.trim_end_matches(['/', '\\'])
    } else {
        trimmed
    };

    if DANGEROUS_ROOTS.iter().any(|dangerous| {
        normalized.eq_ignore_ascii_case(dangerous) || trimmed.eq_ignore_ascii_case(dangerous)
    }) {
        return Err(format!(
            "workspace root is a forbidden system path: {normalized}"
        ));
    }

    // Windows drive root forms like "C:\" already covered; also reject bare "C:".
    if normalized.len() == 2
        && normalized.as_bytes()[0].is_ascii_alphabetic()
        && normalized.as_bytes()[1] == b':'
    {
        return Err(format!(
            "workspace root is a forbidden system path: {normalized}"
        ));
    }

    Ok(())
}

impl ProjectConfig {
    /// Validate local and remote roots before persistence.
    pub fn validate(&self) -> Result<(), String> {
        validate_workspace_root(&self.local_root).map_err(|e| format!("local_root: {e}"))?;
        validate_workspace_root(&self.remote_root).map_err(|e| format!("remote_root: {e}"))?;
        if self.name.trim().is_empty() {
            return Err("project name must not be empty".into());
        }
        if self.ssh_host_alias.trim().is_empty() {
            return Err("ssh_host_alias must not be empty".into());
        }
        Ok(())
    }

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
        self.validate().map_err(std::io::Error::other)?;
        let mut projects = Self::load_raw()?;
        projects.insert(self.id.to_string(), self.clone());
        Self::save_raw(&projects)
    }

    /// List all saved projects.
    pub fn list_all() -> Result<Vec<ProjectConfig>, std::io::Error> {
        let projects = Self::load_raw()?;
        Ok(projects.into_values().collect())
    }

    /// Load a single project by ID from the default projects file.
    pub fn find_by_id(id: &str) -> Result<ProjectConfig, std::io::Error> {
        Self::find_in_map(&Self::load_raw()?, id)
    }

    /// Find a project in an already-loaded map (testable without home config dir).
    pub(crate) fn find_in_map(
        projects: &HashMap<String, ProjectConfig>,
        id: &str,
    ) -> Result<ProjectConfig, std::io::Error> {
        if let Some(project) = projects.get(id) {
            return Ok(project.clone());
        }
        projects
            .values()
            .find(|project| project.id.to_string() == id)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("project not found: {id}"),
                )
            })
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
        for project in projects.values() {
            project.validate().map_err(std::io::Error::other)?;
        }
        let path = Self::projects_file()?;
        let bytes = serde_json::to_vec_pretty(projects).map_err(std::io::Error::other)?;
        atomic_write_private_bytes(&path, &bytes)
    }
}

/// Atomically write `bytes` to `path` with mode 0600 on Unix (temp sibling + rename).
pub fn atomic_write_private_bytes(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".projects-{}.tmp", uuid::Uuid::new_v4()));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let write_result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        #[cfg(unix)]
        {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
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

#[cfg(test)]
mod security_root_and_atomic_write_tests {
    use super::{
        ProjectConfig, SyncConfig, SyncMode, atomic_write_private_bytes, validate_workspace_root,
    };
    use std::path::PathBuf;

    fn sample_project(local_root: &str, remote_root: &str) -> ProjectConfig {
        ProjectConfig {
            id: uuid::Uuid::new_v4(),
            name: "demo".into(),
            ssh_host_alias: "devbox".into(),
            remote_root: remote_root.into(),
            local_root: local_root.into(),
            workspace_id: uuid::Uuid::new_v4(),
            tmux_session: "demo".into(),
            sync: SyncConfig {
                mode: SyncMode::TwoWaySafe,
                includes: vec!["**/*".into()],
                excludes: vec![],
                protect_secrets: true,
            },
        }
    }

    #[test]
    fn rejects_dangerous_root() {
        for root in [
            "",
            "   ",
            "/",
            "/etc",
            "/var",
            "/usr",
            "/bin",
            "/sbin",
            "/System",
            "/private",
            "/etc/",
            "C:\\",
            "C:/",
            "C:",
            "/Users/me/../../etc",
            "/tmp/../etc",
            "relative/../escape",
        ] {
            assert!(
                validate_workspace_root(root).is_err(),
                "expected rejection for {root:?}"
            );
        }

        let project = sample_project("/", "/home/user/project");
        assert!(project.validate().is_err());

        let project = sample_project("/Users/me/project", "/etc");
        assert!(project.validate().is_err());
    }

    #[test]
    fn accepts_safe_project_roots() {
        validate_workspace_root("/Users/me/workspace/app").unwrap();
        validate_workspace_root("/home/dev/code/my-app").unwrap();
        validate_workspace_root("/var/lib/fns-workspace/tenant-a").unwrap();

        let project = sample_project("/Users/me/workspace/app", "/home/dev/code/my-app");
        project.validate().unwrap();
    }

    #[test]
    fn atomic_write_private_bytes_uses_mode_0600_on_unix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.json");
        atomic_write_private_bytes(&path, b"{\"ok\":true}").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "{\"ok\":true}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // No leftover temp siblings.
        let leftovers: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".projects-") && n.ends_with(".tmp"))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }
}
