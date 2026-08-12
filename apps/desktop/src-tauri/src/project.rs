//! Project configuration persistence and path conventions.
//!
//! Only non-sensitive configuration is stored, as JSON under the app config
//! directory. The SSH password lives in the system keychain keyed by project id
//! (see `auth::keychain`), never here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Directory the wizard puts projects under, on both ends.
const PROJECT_NAMESPACE: &str = "cchaven";

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

impl ProjectConfig {
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
        let path = Self::projects_file()?;
        let bytes = serde_json::to_vec_pretty(projects).map_err(std::io::Error::other)?;
        std::fs::write(&path, bytes)
    }
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
