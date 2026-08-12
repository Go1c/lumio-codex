//! 本地凭据存储。见 `.spec/decisions/0001-lumio-credentials-local-file.md`：
//! 本期不引入系统凭据库依赖，落 Lumio 自有数据目录下的 owner-only 文件。
//! 明文令牌与 API Key 只在进程内流转，对外只暴露三态 [`CredentialStatus`]。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::product;

const FILE_NAME: &str = "credentials.json";
const SCHEMA_VERSION: u32 = 1;
const STORAGE_ERROR: &str = "KEY_STORAGE_UNAVAILABLE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialStatus {
    Present,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub api_key: Option<String>,
    pub email: String,
}

/// 磁盘表示。与 [`StoredCredentials`] 分开，`schema_version` 才不会渗进内存模型，
/// 也保证这个结构体不会被无意中序列化到 IPC 边界之外。
#[derive(Debug, Serialize, Deserialize)]
struct StoredRecord {
    schema_version: u32,
    email: String,
    access_token: String,
    refresh_token: String,
    api_key: Option<String>,
}

pub struct CredentialStore {
    root: PathBuf,
}

impl CredentialStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_store() -> anyhow::Result<Self> {
        let root = product::state_dir()
            .ok_or_else(|| anyhow::anyhow!("unable to resolve the Lumio state directory"))?;
        Ok(Self::new(root))
    }

    pub fn path(&self) -> PathBuf {
        self.root.join(FILE_NAME)
    }

    pub fn status(&self) -> CredentialStatus {
        let path = self.path();
        if !path.exists() {
            return CredentialStatus::Missing;
        }
        match read_record(&path) {
            Some(_) => CredentialStatus::Present,
            None => CredentialStatus::Invalid,
        }
    }

    pub fn load(&self) -> Option<StoredCredentials> {
        read_record(&self.path()).map(|record| StoredCredentials {
            access_token: record.access_token,
            refresh_token: record.refresh_token,
            api_key: record.api_key,
            email: record.email,
        })
    }

    pub fn save(&self, credentials: &StoredCredentials) -> Result<(), String> {
        let record = StoredRecord {
            schema_version: SCHEMA_VERSION,
            email: credentials.email.clone(),
            access_token: credentials.access_token.clone(),
            refresh_token: credentials.refresh_token.clone(),
            api_key: credentials.api_key.clone(),
        };
        // 失败原因一律折叠成稳定码：io::Error 的 Display 会带上完整路径。
        let bytes = serde_json::to_vec(&record).map_err(|_| STORAGE_ERROR.to_string())?;
        let path = self.path();
        crate::settings::atomic_write(&path, &bytes).map_err(|_| STORAGE_ERROR.to_string())?;
        restrict_permissions(&path)
    }

    pub fn clear(&self) -> Result<(), String> {
        let path = self.path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(STORAGE_ERROR.to_string()),
        }
    }
}

fn read_record(path: &Path) -> Option<StoredRecord> {
    let raw = std::fs::read_to_string(path).ok()?;
    let record: StoredRecord = serde_json::from_str(&raw).ok()?;
    if record.schema_version != SCHEMA_VERSION {
        return None;
    }
    Some(record)
}

/// `atomic_write` 是 rename 语义，临时文件上的权限不会跟到最终路径，只能事后收紧。
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| STORAGE_ERROR.to_string())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StoredCredentials {
        StoredCredentials {
            access_token: "header.payload.signature".to_string(),
            refresh_token: "rt_abc".to_string(),
            api_key: Some("sk-desktop".to_string()),
            email: "user@example.com".to_string(),
        }
    }

    #[test]
    fn an_empty_store_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());

        assert_eq!(store.status(), CredentialStatus::Missing);
        assert!(store.load().is_none());
    }

    #[test]
    fn saved_credentials_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();

        assert_eq!(store.status(), CredentialStatus::Present);
        let loaded = store.load().unwrap();
        assert_eq!(loaded.access_token, "header.payload.signature");
        assert_eq!(loaded.api_key.as_deref(), Some("sk-desktop"));
        assert_eq!(loaded.email, "user@example.com");
    }

    #[test]
    fn a_corrupted_file_reports_invalid_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();
        std::fs::write(store.path(), b"{ not json").unwrap();

        assert_eq!(store.status(), CredentialStatus::Invalid);
        assert!(store.load().is_none());
    }

    #[test]
    fn a_file_from_a_future_schema_version_reports_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(store.path(), br#"{"schema_version":99,"email":"a@b.c"}"#).unwrap();

        assert_eq!(store.status(), CredentialStatus::Invalid);
    }

    #[test]
    fn clearing_removes_the_file_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();

        store.clear().unwrap();
        assert_eq!(store.status(), CredentialStatus::Missing);
        store.clear().unwrap();
        assert_eq!(store.status(), CredentialStatus::Missing);
    }

    #[test]
    fn saving_replaces_the_previous_record_rather_than_appending() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();
        store
            .save(&StoredCredentials {
                api_key: None,
                ..sample()
            })
            .unwrap();

        assert_eq!(store.load().unwrap().api_key, None);
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert_eq!(raw.matches("\"email\"").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_credential_file_is_only_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save(&sample()).unwrap();

        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "unexpected mode {:o}", mode & 0o777);
    }

    #[test]
    fn the_status_serializes_to_the_three_values_the_ui_expects() {
        assert_eq!(
            serde_json::to_string(&CredentialStatus::Present).unwrap(),
            "\"present\""
        );
        assert_eq!(
            serde_json::to_string(&CredentialStatus::Missing).unwrap(),
            "\"missing\""
        );
        assert_eq!(
            serde_json::to_string(&CredentialStatus::Invalid).unwrap(),
            "\"invalid\""
        );
    }
}
