//! System keychain wrapper.
//!
//! Two classes of secret exist in this app and neither may ever touch disk in
//! cleartext: the OAuth refresh token and the SSH password of a project. Both
//! live in the macOS keychain under the service name below; `projects.json`
//! only stores a reference to the account name.

use std::collections::HashMap;
use std::sync::Mutex;

/// Keychain service name — one entry family for the whole app.
pub const SERVICE: &str = "cn.cchaven.desktop";

/// Keychain account holding the OAuth refresh token.
const ACCOUNT_REFRESH_TOKEN: &str = "oauth-refresh-token";

/// Set to `memory` to keep secrets in-process (headless dev and CI, where the
/// keychain would pop a UI prompt).
const BACKEND_ENV: &str = "CCHAVEN_SECRET_BACKEND";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The keychain is unavailable or the user denied access.
    Backend(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(reason) => write!(f, "系统钥匙串不可用：{reason}"),
        }
    }
}

/// A named secret slot. Implementations must never log secret values.
pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError>;
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// macOS keychain backed store.
pub struct KeychainStore {
    service: String,
}

impl KeychainStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, account).map_err(|e| SecretError::Backend(e.to_string()))
    }
}

impl SecretStore for KeychainStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        self.entry(account)?
            .set_password(secret)
            .map_err(|e| SecretError::Backend(e.to_string()))
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        match self.entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Backend(e.to_string())),
        }
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::Backend(e.to_string())),
        }
    }
}

/// In-process store used by tests and by `CCHAVEN_SECRET_BACKEND=memory`.
#[derive(Default)]
pub struct MemoryStore {
    slots: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemoryStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        let mut slots = self.slots.lock().map_err(poisoned)?;
        slots.insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        let slots = self.slots.lock().map_err(poisoned)?;
        Ok(slots.get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        let mut slots = self.slots.lock().map_err(poisoned)?;
        slots.remove(account);
        Ok(())
    }
}

fn poisoned<T>(_: T) -> SecretError {
    SecretError::Backend("内部锁失效".into())
}

/// Typed accessors over a [`SecretStore`], so callers never spell account names.
pub struct Secrets {
    store: Box<dyn SecretStore>,
}

/// Debug prints the backend only — never a secret.
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets([REDACTED])")
    }
}

impl Secrets {
    pub fn new(store: Box<dyn SecretStore>) -> Self {
        Self { store }
    }

    /// Pick the backend from the environment: the real keychain by default.
    pub fn from_env() -> Self {
        match std::env::var(BACKEND_ENV).as_deref() {
            Ok("memory") => Self::new(Box::new(MemoryStore::new())),
            _ => Self::new(Box::new(KeychainStore::new(SERVICE))),
        }
    }

    pub fn store_refresh_token(&self, token: &str) -> Result<(), SecretError> {
        self.store.set(ACCOUNT_REFRESH_TOKEN, token)
    }

    pub fn refresh_token(&self) -> Result<Option<String>, SecretError> {
        self.store.get(ACCOUNT_REFRESH_TOKEN)
    }

    pub fn clear_refresh_token(&self) -> Result<(), SecretError> {
        self.store.delete(ACCOUNT_REFRESH_TOKEN)
    }

    pub fn store_ssh_password(&self, project_id: &str, password: &str) -> Result<(), SecretError> {
        self.store.set(&ssh_account(project_id), password)
    }

    pub fn ssh_password(&self, project_id: &str) -> Result<Option<String>, SecretError> {
        self.store.get(&ssh_account(project_id))
    }

    pub fn clear_ssh_password(&self, project_id: &str) -> Result<(), SecretError> {
        self.store.delete(&ssh_account(project_id))
    }

    /// Bearer token the local loopback agent expects (workspace-sync-v2).
    pub fn store_sync_agent_token(&self, project_id: &str, token: &str) -> Result<(), SecretError> {
        self.store.set(&sync_agent_account(project_id), token)
    }

    pub fn sync_agent_token(&self, project_id: &str) -> Result<Option<String>, SecretError> {
        self.store.get(&sync_agent_account(project_id))
    }

    pub fn clear_sync_agent_token(&self, project_id: &str) -> Result<(), SecretError> {
        self.store.delete(&sync_agent_account(project_id))
    }
}

/// Account name for a project's SSH password. Project ids are UUIDs, so the
/// namespace cannot collide with the token account.
fn ssh_account(project_id: &str) -> String {
    format!("ssh-password:{project_id}")
}

fn sync_agent_account(project_id: &str) -> String {
    format!("sync-agent-token:{project_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Secrets {
        Secrets::new(Box::new(MemoryStore::new()))
    }

    #[test]
    fn refresh_token_round_trips() {
        let secrets = secrets();
        assert_eq!(secrets.refresh_token(), Ok(None));

        secrets.store_refresh_token("rt-1").expect("store");
        assert_eq!(secrets.refresh_token(), Ok(Some("rt-1".into())));

        // Re-authorising replaces rather than appends.
        secrets.store_refresh_token("rt-2").expect("store");
        assert_eq!(secrets.refresh_token(), Ok(Some("rt-2".into())));
    }

    #[test]
    fn logout_clears_the_refresh_token() {
        let secrets = secrets();
        secrets.store_refresh_token("rt").expect("store");
        secrets.clear_refresh_token().expect("clear");
        assert_eq!(secrets.refresh_token(), Ok(None));
        // Clearing twice is not an error: logout may race with a failed refresh.
        secrets.clear_refresh_token().expect("clear again");
    }

    #[test]
    fn ssh_passwords_are_isolated_per_project() {
        let secrets = secrets();
        secrets.store_ssh_password("a", "pw-a").expect("store");
        secrets.store_ssh_password("b", "pw-b").expect("store");

        assert_eq!(secrets.ssh_password("a"), Ok(Some("pw-a".into())));
        assert_eq!(secrets.ssh_password("b"), Ok(Some("pw-b".into())));

        secrets.clear_ssh_password("a").expect("clear");
        assert_eq!(secrets.ssh_password("a"), Ok(None));
        assert_eq!(secrets.ssh_password("b"), Ok(Some("pw-b".into())));
    }

    #[test]
    fn ssh_account_names_cannot_collide_with_the_token_slot() {
        assert_ne!(ssh_account("oauth-refresh-token"), ACCOUNT_REFRESH_TOKEN);
        assert_eq!(ssh_account("p1"), "ssh-password:p1");
    }

    #[test]
    fn debug_output_never_carries_a_secret() {
        let secrets = secrets();
        secrets.store_refresh_token("super-secret").expect("store");
        assert_eq!(format!("{secrets:?}"), "Secrets([REDACTED])");
    }
}
