//! 官方 Codex 配置接管。Lumio 只拥有 `config.toml` 的 `model` / `model_provider` /
//! `model_providers.lumio` 与 `auth.json` 的 `OPENAI_API_KEY`，其余内容一律原样保留；
//! 首次接管前的原始字节存快照，恢复时整体写回。
//!
//! 「是否首次接管」只看快照本身([`SnapshotSlot`])，绝不看 manifest：manifest 是整个流程
//! 最后一步才写的，用它判断等于把「写完 config / auth 但还没写 manifest 就被打断」误判成
//! 从未接管过，下一次接管会把已被 Lumio 改写的内容当成原始状态存进快照。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::settings::atomic_write;

const WRITE_FAILED: &str = "CODEX_CONFIG_WRITE_FAILED";
const CONFLICT: &str = "CODEX_CONFIG_CONFLICT";
const RESTORE_FAILED: &str = "CODEX_RESTORE_FAILED";

const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
const TAKEOVER_DIR: &str = "takeover";
const MANIFEST_FILE: &str = "manifest.json";
const CONFIG_SNAPSHOT: &str = "config.toml.snapshot";
const AUTH_SNAPSHOT: &str = "auth.json.snapshot";
const CONFIG_ABSENT_MARKER: &str = "config.toml.absent";
const AUTH_ABSENT_MARKER: &str = "auth.json.absent";

const PROVIDER_ID: &str = "lumio";
const AUTH_KEY_FIELD: &str = "OPENAI_API_KEY";

#[derive(Debug, Clone)]
pub struct TakeoverRequest {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeoverRecord {
    pub applied_at: String,
    pub config_sha256: String,
    pub auth_sha256: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeoverHealth {
    NotApplied,
    Healthy,
    Conflicted { error_code: String },
}

/// 接管后的自检记录。**只记 Lumio 写下去的内容**（用于发现外部改动）；
/// 「接管前是什么样」一律由 [`SnapshotSlot`] 持有，不在这里留第二份真值。
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    config_sha256: String,
    auth_sha256: String,
    applied_at: String,
    model: String,
}

#[derive(Debug)]
enum ManifestState {
    Missing,
    Corrupt,
    Loaded(Manifest),
}

/// 一个被接管文件的「接管前原始状态」。`<name>.snapshot` 存原始字节；
/// `<name>.absent` 记录「接管前这个文件不存在」——这条信息也必须落在快照目录里，
/// 否则 manifest 一丢就分不清「原本没有」和「从未接管」。
struct SnapshotSlot {
    content: PathBuf,
    absent: PathBuf,
}

/// 已记录的接管前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginalState {
    Present,
    Absent,
}

impl SnapshotSlot {
    fn config(takeover_dir: &Path) -> Self {
        Self {
            content: takeover_dir.join(CONFIG_SNAPSHOT),
            absent: takeover_dir.join(CONFIG_ABSENT_MARKER),
        }
    }

    fn auth(takeover_dir: &Path) -> Self {
        Self {
            content: takeover_dir.join(AUTH_SNAPSHOT),
            absent: takeover_dir.join(AUTH_ABSENT_MARKER),
        }
    }

    /// `None` 表示这个槽位还没记录过任何东西，也就是「本次是首次接管」。
    fn recorded(&self) -> Option<OriginalState> {
        if self.content.exists() {
            Some(OriginalState::Present)
        } else if self.absent.exists() {
            Some(OriginalState::Absent)
        } else {
            None
        }
    }

    /// 首次接管时把原始状态记下来；已经记过就原样保留——快照一旦建立便不可再被覆盖。
    fn record_once(&self, current: Option<&[u8]>) -> Result<(), String> {
        if self.recorded().is_some() {
            return Ok(());
        }
        match current {
            Some(bytes) => write_bytes(&self.content, bytes, WRITE_FAILED),
            None => write_bytes(&self.absent, b"", WRITE_FAILED),
        }
    }

    fn discard(&self) -> Result<(), String> {
        remove_if_present(&self.content)?;
        remove_if_present(&self.absent)
    }
}

pub fn apply_takeover(
    codex_home: &Path,
    state_dir: &Path,
    request: &TakeoverRequest,
) -> Result<TakeoverRecord, String> {
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);

    let config_bytes = read_optional(&config_path)?;
    let auth_bytes = read_optional(&auth_path)?;

    // 先把两份内容解析并编辑到内存里；任何解析失败都在落盘之前返回，
    // 用户原本的文件（哪怕本来就是坏的）不会被我们改写。
    let config_output = render_config(config_bytes.as_deref(), request)?;
    let auth_output = render_auth(auth_bytes.as_deref(), &request.api_key)?;

    let takeover_dir = state_dir.join(TAKEOVER_DIR);
    let manifest_path = takeover_dir.join(MANIFEST_FILE);
    // 用户的文件在这两行之前一个字节都没被碰过：原始状态先不可逆地记下来，再动手改写。
    SnapshotSlot::config(&takeover_dir).record_once(config_bytes.as_deref())?;
    SnapshotSlot::auth(&takeover_dir).record_once(auth_bytes.as_deref())?;

    write_bytes(&config_path, config_output.as_bytes(), WRITE_FAILED)?;
    write_bytes(&auth_path, auth_output.as_bytes(), WRITE_FAILED)?;
    restrict_permissions(&auth_path)?;

    let manifest = Manifest {
        config_sha256: sha256(config_output.as_bytes()),
        auth_sha256: sha256(auth_output.as_bytes()),
        applied_at: now_unix_seconds(),
        model: request.model.clone(),
    };
    let encoded = serde_json::to_vec(&manifest).map_err(|_| WRITE_FAILED.to_string())?;
    write_bytes(&manifest_path, &encoded, WRITE_FAILED)?;

    Ok(TakeoverRecord {
        applied_at: manifest.applied_at,
        config_sha256: manifest.config_sha256,
        auth_sha256: manifest.auth_sha256,
        model: manifest.model,
    })
}

pub fn check_takeover(codex_home: &Path, state_dir: &Path) -> TakeoverHealth {
    let takeover_dir = state_dir.join(TAKEOVER_DIR);
    let snapshot_taken = SnapshotSlot::config(&takeover_dir).recorded().is_some()
        || SnapshotSlot::auth(&takeover_dir).recorded().is_some();

    let manifest = match load_manifest(&takeover_dir.join(MANIFEST_FILE)) {
        ManifestState::Loaded(manifest) => manifest,
        // 快照在但记录不完整（manifest 没写成或写坏了）：接管确实动过用户的文件，只是
        // 无法核对当前内容。报 NotApplied 会让 provisioning 静默重新接管，报冲突才诚实——
        // 它把 restore 入口露给用户，而快照还完好地躺在旁边。
        ManifestState::Corrupt => {
            return TakeoverHealth::Conflicted {
                error_code: CONFLICT.to_string(),
            };
        }
        ManifestState::Missing if snapshot_taken => {
            return TakeoverHealth::Conflicted {
                error_code: CONFLICT.to_string(),
            };
        }
        ManifestState::Missing => return TakeoverHealth::NotApplied,
    };

    for (path, expected) in [
        (codex_home.join(CONFIG_FILE), &manifest.config_sha256),
        (codex_home.join(AUTH_FILE), &manifest.auth_sha256),
    ] {
        match std::fs::read(&path) {
            Ok(bytes) if &sha256(&bytes) == expected => {}
            _ => {
                return TakeoverHealth::Conflicted {
                    error_code: CONFLICT.to_string(),
                };
            }
        }
    }

    TakeoverHealth::Healthy
}

/// 只依赖快照，不依赖 manifest：manifest 缺失或损坏时，只要原始字节还在就必须能恢复。
pub fn restore(codex_home: &Path, state_dir: &Path) -> Result<(), String> {
    let takeover_dir = state_dir.join(TAKEOVER_DIR);
    let config_slot = SnapshotSlot::config(&takeover_dir);
    let auth_slot = SnapshotSlot::auth(&takeover_dir);
    let config_state = config_slot.recorded();
    let auth_state = auth_slot.recorded();
    if config_state.is_none() && auth_state.is_none() {
        return Err(RESTORE_FAILED.to_string());
    }

    // 只恢复记录过的槽位。没记录 = 接管还没轮到这个文件，它现在就是原始内容。
    if let Some(state) = config_state {
        restore_one(&codex_home.join(CONFIG_FILE), &config_slot, state)?;
    }
    if let Some(state) = auth_state {
        restore_one(&codex_home.join(AUTH_FILE), &auth_slot, state)?;
    }

    // 快照在成功写回之后才丢弃，中途失败仍留有原始字节可再试一次。
    remove_if_present(&takeover_dir.join(MANIFEST_FILE))?;
    config_slot.discard()?;
    auth_slot.discard()?;
    let _ = std::fs::remove_dir(&takeover_dir);
    Ok(())
}

fn restore_one(target: &Path, slot: &SnapshotSlot, state: OriginalState) -> Result<(), String> {
    match state {
        OriginalState::Present => {
            let bytes = std::fs::read(&slot.content).map_err(|_| RESTORE_FAILED.to_string())?;
            write_bytes(target, &bytes, RESTORE_FAILED)
        }
        OriginalState::Absent => remove_if_present(target),
    }
}

fn render_config(existing: Option<&[u8]>, request: &TakeoverRequest) -> Result<String, String> {
    let text = match existing {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| WRITE_FAILED.to_string())?
            .to_string(),
        None => String::new(),
    };
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| WRITE_FAILED.to_string())?;

    doc["model"] = toml_edit::value(request.model.as_str());
    doc["model_provider"] = toml_edit::value(PROVIDER_ID);
    let provider = &mut doc["model_providers"][PROVIDER_ID];
    provider["name"] = toml_edit::value(super::product::PRODUCT_NAME);
    provider["base_url"] = toml_edit::value(request.base_url.as_str());
    provider["wire_api"] = toml_edit::value("responses");
    provider["env_key"] = toml_edit::value(AUTH_KEY_FIELD);

    Ok(doc.to_string())
}

fn render_auth(existing: Option<&[u8]>, api_key: &str) -> Result<String, String> {
    let mut value = match existing {
        Some(bytes) if !bytes.is_empty() => serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|_| WRITE_FAILED.to_string())?,
        _ => serde_json::Value::Object(serde_json::Map::new()),
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| WRITE_FAILED.to_string())?;
    object.insert(
        AUTH_KEY_FIELD.to_string(),
        serde_json::Value::String(api_key.to_string()),
    );
    serde_json::to_string_pretty(&value).map_err(|_| WRITE_FAILED.to_string())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(WRITE_FAILED.to_string()),
    }
}

/// 「读不到」与「读到了但解析不了」必须分开：`atomic_write` 不做 fsync，manifest 自身
/// 是可能写坏的，而把损坏当成缺失就会静默重新接管。
fn load_manifest(path: &Path) -> ManifestState {
    let Ok(bytes) = std::fs::read(path) else {
        return ManifestState::Missing;
    };
    match serde_json::from_slice(&bytes) {
        Ok(manifest) => ManifestState::Loaded(manifest),
        Err(_) => ManifestState::Corrupt,
    }
}

fn write_bytes(path: &Path, bytes: &[u8], error_code: &str) -> Result<(), String> {
    atomic_write(path, bytes).map_err(|_| error_code.to_string())
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RESTORE_FAILED.to_string()),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn now_unix_seconds() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// `auth.json` 里是 API Key 明文，写完必须收紧到 owner-only；
/// `atomic_write` 是 rename 语义，权限只能落在最终路径上。
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| WRITE_FAILED.to_string())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> TakeoverRequest {
        TakeoverRequest {
            model: "gpt-example".to_string(),
            api_key: "sk-desktop".to_string(),
            base_url: "https://api.lumio.games/v1".to_string(),
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        codex_home: std::path::PathBuf,
        state_dir: std::path::PathBuf,
    }

    impl Fixture {
        fn manifest_path(&self) -> std::path::PathBuf {
            self.state_dir.join(TAKEOVER_DIR).join(MANIFEST_FILE)
        }
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let codex_home = root.path().join("codex-home");
        let state_dir = root.path().join("state");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        Fixture {
            _root: root,
            codex_home,
            state_dir,
        }
    }

    #[test]
    fn takeover_writes_only_the_fields_lumio_owns() {
        let fx = fixture();
        std::fs::write(
            fx.codex_home.join("config.toml"),
            "model = \"user-choice\"\n\n[mcp_servers.mine]\ncommand = \"keep-me\"\n\n[projects.\"/tmp/x\"]\ntrust_level = \"trusted\"\n",
        )
        .unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        let written = std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap();
        assert!(written.contains("gpt-example"));
        assert!(
            written.contains("keep-me"),
            "user-owned section was dropped:\n{written}"
        );
        assert!(
            written.contains("trust_level"),
            "user projects were dropped:\n{written}"
        );
    }

    #[test]
    fn takeover_snapshots_the_original_bytes_before_the_first_write() {
        let fx = fixture();
        let original = "model = \"user-choice\"\n";
        std::fs::write(fx.codex_home.join("config.toml"), original).unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn the_snapshot_is_taken_once_and_survives_a_second_takeover() {
        let fx = fixture();
        let original = "model = \"user-choice\"\n";
        std::fs::write(fx.codex_home.join("config.toml"), original).unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        let second = TakeoverRequest {
            model: "gpt-other".to_string(),
            ..request()
        };
        apply_takeover(&fx.codex_home, &fx.state_dir, &second).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            original,
            "the second takeover overwrote the pre-takeover snapshot"
        );
    }

    /// 首次接管在写完 config / auth 之后、写 manifest 之前被打断：磁盘上留着正确的原始
    /// 快照，但没有 manifest。下一次接管不得把「已被 Lumio 改写」的内容当成原始状态。
    #[test]
    fn an_interrupted_first_takeover_keeps_the_original_bytes_across_the_next_takeover() {
        let fx = fixture();
        let original_config = "model = \"user-choice\"\n";
        let original_auth = r#"{"tokens":{"id_token":"user-chatgpt-token"}}"#;
        std::fs::write(fx.codex_home.join("config.toml"), original_config).unwrap();
        std::fs::write(fx.codex_home.join("auth.json"), original_auth).unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::remove_file(fx.manifest_path()).unwrap();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            original_config,
            "the takeover that followed the interruption overwrote the snapshot"
        );
        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("auth.json")).unwrap(),
            original_auth,
            "the user's own auth.json was lost"
        );
    }

    #[test]
    fn restoring_works_from_the_snapshot_alone_when_the_manifest_is_gone() {
        let fx = fixture();
        let original = "model = \"user-choice\"\n";
        std::fs::write(fx.codex_home.join("config.toml"), original).unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::remove_file(fx.manifest_path()).unwrap();

        restore(&fx.codex_home, &fx.state_dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            original
        );
    }

    /// 「接管前这个文件不存在」也是原始状态的一部分，不能只存在 manifest 里。
    #[test]
    fn a_file_that_did_not_exist_is_still_removed_after_the_manifest_is_lost() {
        let fx = fixture();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::remove_file(fx.manifest_path()).unwrap();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        assert!(
            !fx.codex_home.join("config.toml").exists(),
            "a file that did not exist before takeover was left behind"
        );
        assert!(!fx.codex_home.join("auth.json").exists());
    }

    /// manifest 自己写坏时报 `NotApplied` 会让 provisioning 静默重新接管；
    /// 报冲突才能把 restore 入口露给用户。
    #[test]
    fn a_corrupt_manifest_is_reported_as_a_conflict_rather_than_never_applied() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::write(fx.manifest_path(), b"{ not json").unwrap();

        match check_takeover(&fx.codex_home, &fx.state_dir) {
            TakeoverHealth::Conflicted { error_code } => {
                assert_eq!(error_code, "CODEX_CONFIG_CONFLICT");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn an_interrupted_takeover_is_not_reported_as_never_applied() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::remove_file(fx.manifest_path()).unwrap();

        assert!(
            !matches!(
                check_takeover(&fx.codex_home, &fx.state_dir),
                TakeoverHealth::NotApplied
            ),
            "an interrupted takeover reported as never applied would be taken over again silently"
        );
    }

    #[test]
    fn a_corrupt_manifest_does_not_let_the_next_takeover_overwrite_the_snapshot() {
        let fx = fixture();
        let original = "model = \"user-choice\"\n";
        std::fs::write(fx.codex_home.join("config.toml"), original).unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::write(fx.manifest_path(), b"{ not json").unwrap();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn restoring_removes_files_that_did_not_exist_before_takeover() {
        let fx = fixture();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        assert!(fx.codex_home.join("config.toml").exists());

        restore(&fx.codex_home, &fx.state_dir).unwrap();
        assert!(!fx.codex_home.join("config.toml").exists());
        assert!(!fx.codex_home.join("auth.json").exists());
    }

    #[test]
    fn health_reports_not_applied_before_any_takeover() {
        let fx = fixture();
        assert!(matches!(
            check_takeover(&fx.codex_home, &fx.state_dir),
            TakeoverHealth::NotApplied
        ));
    }

    #[test]
    fn health_is_clean_right_after_a_takeover() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        assert!(matches!(
            check_takeover(&fx.codex_home, &fx.state_dir),
            TakeoverHealth::Healthy
        ));
    }

    #[test]
    fn an_external_edit_after_takeover_is_reported_as_a_conflict() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::write(
            fx.codex_home.join("config.toml"),
            "model = \"someone-else\"\n",
        )
        .unwrap();

        match check_takeover(&fx.codex_home, &fx.state_dir) {
            TakeoverHealth::Conflicted { error_code } => {
                assert_eq!(error_code, "CODEX_CONFIG_CONFLICT");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn deleting_the_managed_config_after_takeover_is_also_a_conflict() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        std::fs::remove_file(fx.codex_home.join("config.toml")).unwrap();

        assert!(matches!(
            check_takeover(&fx.codex_home, &fx.state_dir),
            TakeoverHealth::Conflicted { .. }
        ));
    }

    #[test]
    fn the_api_key_lands_in_the_official_auth_file_and_is_removed_on_restore() {
        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        let auth = std::fs::read_to_string(fx.codex_home.join("auth.json")).unwrap();
        assert!(auth.contains("sk-desktop"));

        restore(&fx.codex_home, &fx.state_dir).unwrap();
        assert!(!fx.codex_home.join("auth.json").exists());
    }

    #[test]
    fn restoring_keeps_unrelated_auth_fields_that_existed_before_takeover() {
        let fx = fixture();
        std::fs::write(
            fx.codex_home.join("auth.json"),
            r#"{"OPENAI_API_KEY":"user-key","tokens":{"id_token":"keep"}}"#,
        )
        .unwrap();

        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();
        restore(&fx.codex_home, &fx.state_dir).unwrap();

        let auth = std::fs::read_to_string(fx.codex_home.join("auth.json")).unwrap();
        assert!(auth.contains("user-key"));
        assert!(auth.contains("keep"));
        assert!(!auth.contains("sk-desktop"));
    }

    #[cfg(unix)]
    #[test]
    fn the_auth_file_written_by_takeover_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let fx = fixture();
        apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap();

        let mode = std::fs::metadata(fx.codex_home.join("auth.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn restore_without_a_snapshot_reports_a_stable_error_code() {
        let fx = fixture();
        assert_eq!(
            restore(&fx.codex_home, &fx.state_dir).unwrap_err(),
            "CODEX_RESTORE_FAILED"
        );
    }

    #[test]
    fn invalid_existing_toml_fails_without_destroying_the_users_file() {
        let fx = fixture();
        let broken = "this is [not valid toml\n";
        std::fs::write(fx.codex_home.join("config.toml"), broken).unwrap();

        let error = apply_takeover(&fx.codex_home, &fx.state_dir, &request()).unwrap_err();

        assert_eq!(error, "CODEX_CONFIG_WRITE_FAILED");
        assert_eq!(
            std::fs::read_to_string(fx.codex_home.join("config.toml")).unwrap(),
            broken
        );
    }
}
