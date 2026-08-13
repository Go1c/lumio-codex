//! Project configuration persistence and path conventions.
//!
//! Only non-sensitive configuration is stored, as JSON under the app config
//! directory. The SSH password lives in the system keychain keyed by project id
//! (see `auth::keychain`), never here.
//!
//! A project describes its server as a [`ServerConfig`] (host / user / port /
//! auth method) rather than as a bare `~/.ssh/config` alias: the wizard has to
//! work for users who have never written an SSH config. Everything that shells
//! out to `ssh` addresses the host through [`ProjectConfig::ssh_host_alias`],
//! which yields the config alias when there is one and `user@host` otherwise.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Directory the wizard puts projects under, on both ends.
const PROJECT_NAMESPACE: &str = "cchaven";

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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    #[default]
    TwoWaySafe,
}

/// How to authenticate to the server. Password is the zero-knowledge default;
/// keys are behind 「高级选项」.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    #[default]
    Password,
    Key,
    /// Host defined in `~/.ssh/config`; credentials handled by OpenSSH itself.
    SshConfig,
}

/// Server connection settings captured in wizard step 1.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub host: String,
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub auth: AuthMethod,
    /// Private key path when `auth = key`; never the key material itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// Alias from `~/.ssh/config` when `auth = ssh_config`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_alias: Option<String>,
}

impl ServerConfig {
    /// `user@host`, the chip shown in the workspace top bar.
    pub fn ssh_target(&self) -> String {
        if let Some(alias) = &self.config_alias
            && self.auth == AuthMethod::SshConfig
        {
            return alias.clone();
        }
        format!("{}@{}", self.user, self.host)
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            user: default_user(),
            port: default_port(),
            auth: AuthMethod::default(),
            key_path: None,
            config_alias: None,
        }
    }
}

fn default_user() -> String {
    "root".into()
}

fn default_port() -> u16 {
    22
}

/// Sync configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfig {
    #[serde(default)]
    pub mode: SyncMode,
    #[serde(default = "default_includes")]
    pub includes: Vec<String>,
    #[serde(default = "default_excludes")]
    pub excludes: Vec<String>,
    /// Always true. M3 exposes no way to turn secret protection off; the field
    /// exists because `fns-fs` reads it, not because the UI offers a switch.
    #[serde(default = "protect_secrets_default")]
    pub protect_secrets: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            mode: SyncMode::TwoWaySafe,
            includes: default_includes(),
            excludes: default_excludes(),
            protect_secrets: true,
        }
    }
}

fn default_includes() -> Vec<String> {
    vec!["**".into()]
}

/// Pre-filled exclude rules from 5.3 第 2 步.
fn default_excludes() -> Vec<String> {
    vec![
        ".git/".into(),
        "node_modules/".into(),
        "target/".into(),
        ".env".into(),
    ]
}

fn protect_secrets_default() -> bool {
    true
}

/// Project configuration persisted on the Mac.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub id: uuid::Uuid,
    pub name: String,
    #[serde(default)]
    pub server: ServerConfig,
    pub remote_root: String,
    pub local_root: String,
    #[serde(default = "uuid::Uuid::new_v4")]
    pub workspace_id: uuid::Uuid,
    #[serde(default)]
    pub tmux_session: String,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub created_at: String,
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
    /// How every `ssh` invocation addresses this project's server.
    ///
    /// A `~/.ssh/config` alias when the user picked one, `user@host` otherwise —
    /// the wizard's password path has no config entry to point at.
    pub fn ssh_host_alias(&self) -> String {
        self.server.ssh_target()
    }

    /// Validate local and remote roots before persistence.
    pub fn validate(&self) -> Result<(), String> {
        validate_workspace_root(&self.local_root).map_err(|e| format!("local_root: {e}"))?;
        validate_workspace_root(&self.remote_root).map_err(|e| format!("remote_root: {e}"))?;
        if self.name.trim().is_empty() {
            return Err("project name must not be empty".into());
        }
        if self.server.auth == AuthMethod::SshConfig {
            if self
                .server
                .config_alias
                .as_deref()
                .is_none_or(|alias| alias.trim().is_empty())
            {
                return Err("server.configAlias must not be empty".into());
            }
        } else if self.server.host.trim().is_empty() {
            return Err("server.host must not be empty".into());
        }
        Ok(())
    }

    /// Config directory for storing projects.
    pub fn config_dir() -> Result<PathBuf, std::io::Error> {
        let dir = directories::BaseDirs::new()
            .ok_or_else(|| std::io::Error::other("no config dir"))?
            .config_dir()
            .join("cchaven");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn projects_file() -> Result<PathBuf, std::io::Error> {
        Ok(Self::config_dir()?.join("projects.json"))
    }

    /// Persist (insert or replace) this project.
    pub fn save_to_default(&self) -> Result<(), std::io::Error> {
        self.validate().map_err(std::io::Error::other)?;
        let mut projects = Self::load_raw()?;
        projects.insert(self.id.to_string(), self.clone());
        Self::save_raw(&projects)
    }

    /// All saved projects, ordered by creation time then name so the sidebar
    /// does not reshuffle between launches.
    pub fn list_all() -> Result<Vec<ProjectConfig>, std::io::Error> {
        let mut projects: Vec<_> = Self::load_raw()?.into_values().collect();
        projects.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.name.cmp(&b.name)));
        Ok(projects)
    }

    pub fn get(id: &str) -> Result<Option<ProjectConfig>, std::io::Error> {
        Ok(Self::load_raw()?.remove(id))
    }

    /// Load a single project by ID, erroring when it is gone. The engineering
    /// surfaces (deploy, monitor, sync) want the error, not an `Option`.
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

    /// Remove a project from the app. Files on both ends are left untouched.
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

/// Stable per-install device id reported to the control plane.
pub fn device_id() -> String {
    let Ok(dir) = ProjectConfig::config_dir() else {
        return "unknown-device".into();
    };
    let path = dir.join("device-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let generated = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::write(&path, &generated);
    generated
}

/// Turn a display name into a path segment (5.3 目录自动预设).
///
/// Users type things like 「我的项目 v2」; the derived remote path has to stay a
/// well-behaved POSIX segment without surprising the user with an empty name.
pub fn project_slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            slug.push(ch);
            last_dash = false;
        } else if !ch.is_whitespace() && !ch.is_ascii() {
            // Keep CJK and other non-ASCII characters: they are valid in paths
            // and users expect to recognise their project on the server.
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches(['-', '.'].as_slice()).to_string();
    if slug.is_empty() {
        "my-project".into()
    } else {
        slug
    }
}

/// Remote project directory preset: `root` lives in `/root`, everyone else in
/// `/home/{user}` (5.3 第 2 步).
pub fn default_remote_root(user: &str, name: &str) -> String {
    let user = user.trim();
    let base = if user.is_empty() || user == "root" {
        "/root".to_string()
    } else {
        format!("/home/{user}")
    };
    format!("{base}/{PROJECT_NAMESPACE}/{}", project_slug(name))
}

/// Local sync folder preset: `~/CCHaven/{name}`.
pub fn default_local_root(home: &Path, name: &str) -> String {
    home.join("CCHaven")
        .join(project_slug(name))
        .to_string_lossy()
        .into_owned()
}

/// tmux session name for a project; sanitised where it is used, kept readable here.
pub fn default_tmux_session(name: &str) -> String {
    format!("cchaven-{}", project_slug(name))
}

/// Force the invariants M3 promises regardless of what came off disk or the UI.
pub fn normalise_sync(mut sync: SyncConfig) -> SyncConfig {
    sync.protect_secrets = true;
    sync.mode = SyncMode::TwoWaySafe;
    if sync.includes.is_empty() {
        sync.includes = default_includes();
    }
    sync
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_projects_live_under_slash_root() {
        assert_eq!(
            default_remote_root("root", "my-project"),
            "/root/cchaven/my-project"
        );
    }

    #[test]
    fn other_users_get_a_home_directory_path() {
        assert_eq!(
            default_remote_root("ubuntu", "api"),
            "/home/ubuntu/cchaven/api"
        );
        assert_eq!(
            default_remote_root("  ec2-user  ", "api"),
            "/home/ec2-user/cchaven/api"
        );
    }

    #[test]
    fn a_missing_user_falls_back_to_the_root_preset() {
        assert_eq!(default_remote_root("", "api"), "/root/cchaven/api");
    }

    #[test]
    fn names_are_reduced_to_safe_path_segments() {
        assert_eq!(project_slug("my project"), "my-project");
        assert_eq!(project_slug("  spaced  out  "), "spaced-out");
        assert_eq!(project_slug("a/b/c"), "a-b-c");
        assert_eq!(project_slug("我的项目"), "我的项目");
        assert_eq!(project_slug(""), "my-project");
        assert_eq!(project_slug("///"), "my-project");
    }

    #[test]
    fn slugs_never_escape_their_parent_directory() {
        for name in ["../../etc", "..", "./..", "a/../../b"] {
            let remote = default_remote_root("root", name);
            assert!(
                remote.starts_with("/root/cchaven/"),
                "{name} produced {remote}"
            );
            assert!(!remote.contains("/../"), "{name} produced {remote}");
        }
    }

    #[test]
    fn local_preset_lives_under_the_home_cchaven_folder() {
        assert_eq!(
            default_local_root(Path::new("/Users/mary"), "my project"),
            "/Users/mary/CCHaven/my-project"
        );
    }

    #[test]
    fn tmux_session_is_derived_from_the_project_name() {
        assert_eq!(default_tmux_session("my project"), "cchaven-my-project");
    }

    #[test]
    fn sync_defaults_protect_secrets_and_preset_excludes() {
        let sync = SyncConfig::default();
        assert!(sync.protect_secrets);
        assert_eq!(sync.mode, SyncMode::TwoWaySafe);
        assert_eq!(
            sync.excludes,
            vec![".git/", "node_modules/", "target/", ".env"]
        );
    }

    #[test]
    fn protect_secrets_stays_on_even_if_a_config_file_says_otherwise() {
        // Defence in depth: a hand-edited config must not disable protection.
        let config: SyncConfig =
            serde_json::from_str(r#"{"protectSecrets": false}"#).expect("parse");
        assert!(
            !config.protect_secrets,
            "field parses as written; enforcement happens in normalise()"
        );
        assert!(normalise_sync(config).protect_secrets);
    }

    #[test]
    fn ssh_target_prefers_the_config_alias_when_that_is_the_auth_method() {
        let mut server = ServerConfig {
            host: "43.156.20.8".into(),
            user: "root".into(),
            port: 22,
            auth: AuthMethod::Password,
            key_path: None,
            config_alias: Some("prod".into()),
        };
        assert_eq!(server.ssh_target(), "root@43.156.20.8");
        server.auth = AuthMethod::SshConfig;
        assert_eq!(server.ssh_target(), "prod");
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

#[cfg(test)]
mod security_root_and_atomic_write_tests {
    use super::{
        AuthMethod, ProjectConfig, ServerConfig, SyncConfig, atomic_write_private_bytes,
        validate_workspace_root,
    };
    use std::path::PathBuf;

    fn sample_project(local_root: &str, remote_root: &str) -> ProjectConfig {
        ProjectConfig {
            id: uuid::Uuid::new_v4(),
            name: "demo".into(),
            server: ServerConfig {
                host: "43.156.20.8".into(),
                user: "root".into(),
                port: 22,
                auth: AuthMethod::Password,
                key_path: None,
                config_alias: None,
            },
            remote_root: remote_root.into(),
            local_root: local_root.into(),
            workspace_id: uuid::Uuid::new_v4(),
            tmux_session: "demo".into(),
            sync: SyncConfig::default(),
            created_at: "0".into(),
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
    fn a_project_without_a_reachable_server_is_rejected() {
        let mut project = sample_project("/Users/me/workspace/app", "/home/dev/code/my-app");
        project.server.host = "  ".into();
        assert!(project.validate().is_err());

        // An ssh_config project is addressed by alias, so a blank host is fine
        // but a blank alias is not.
        project.server.auth = AuthMethod::SshConfig;
        assert!(project.validate().is_err());
        project.server.config_alias = Some("prod".into());
        project.validate().unwrap();
        assert_eq!(project.ssh_host_alias(), "prod");
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
