//! In-process workspace sync: engine, watcher, and reconnecting transport session.
//!
//! 交互设计 6.3 的四态与退避倒计时都从这里读出；没有 agent 端点时如实报离线，
//! 绝不伪造「正在同步」进度。Linux agent 的交叉编译与部署仍见 `docs/spec-gaps.md`.

mod backoff;
mod driver;
mod status;

pub use backoff::RetryLadder;
pub use driver::{
    CredentialSource, RemoteDriver, SessionContext, SessionDriver, SessionOutcome, supervise,
};
pub use status::{EngineProgress, SyncSnapshot, SyncState, SyncStatus, engine_progress};

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use fns_fs::{
    EventCoalescer, FsChange, PlatformWatcher, PriorEntryLookup, RootedWorkspace, SyncRuleConfig,
    SyncRules, WatchMessage, start_platform_watcher,
};
use fns_platform::SecretToken;
use fns_protocol::{
    ClientId, ConflictId, RequiredNullable, WorkspaceConflictChoice,
    WorkspaceConflictCreatedMessage, WorkspaceConflictKind, WorkspaceId, WorkspacePath,
};
use fns_sync_core::{ConflictStatus, SyncEngine, SyncEngineConfig};
use fns_transport::{EngineHandle, EngineWorker, WorkspaceEndpoint};

use crate::auth::keychain::Secrets;
use crate::conflicts::{Conflict, ConflictKind, ConflictSide, Resolution};
use crate::files;
use crate::project::ProjectConfig;

/// Status-bar / activity-panel detail strings. Stable, non-sensitive, English keys
/// kept short so support logs stay greppable; the UI may map them later.
pub const DETAIL_STARTING: &str = "starting";
pub const DETAIL_CONNECTING: &str = "connecting";
pub const DETAIL_NO_ENDPOINT: &str = "no_endpoint";
pub const DETAIL_UNREACHABLE: &str = "unreachable";
pub const DETAIL_UNAUTHORIZED: &str = "unauthorized";
pub const DETAIL_FORBIDDEN: &str = "forbidden";
pub const DETAIL_BAD_ENDPOINT: &str = "bad_endpoint";
pub const DETAIL_PROTOCOL: &str = "protocol";
pub const DETAIL_CLOSED: &str = "closed";
pub const DETAIL_DROPPED: &str = "dropped";

/// One open project's engine worker, watcher, and session supervisor.
///
/// Field order matters for `Drop` (see [`ProjectSession::drop`]): abort the
/// supervisor and drop every `EngineHandle` before `_worker` joins.
struct ProjectSession {
    snapshot: Arc<Mutex<SyncSnapshot>>,
    shutdown: CancellationToken,
    supervisor: Option<tokio::task::JoinHandle<()>>,
    /// Dropped before `_worker` so the engine channel can close.
    engine: EngineHandle,
    _worker: EngineWorker,
    _watcher: Option<PlatformWatcher>,
}

impl Drop for ProjectSession {
    fn drop(&mut self) {
        drop(self._watcher.take());
        self.shutdown.cancel();
        if let Some(task) = self.supervisor.take() {
            task.abort();
        }
    }
}

/// Hosts zero or more per-project sync sessions inside the desktop process.
pub struct SyncManager {
    sessions: Mutex<HashMap<String, ProjectSession>>,
    /// Root under which each project gets `{project_id}/` for sqlite + client id.
    state_root: PathBuf,
    secrets: Arc<Secrets>,
    driver: Arc<dyn SessionDriver>,
    /// Native FS watcher. Tests turn this off to avoid parallel notify deadlocks.
    watch: bool,
}

impl SyncManager {
    /// Production manager: remote transport driver, secrets from the environment.
    pub fn from_env() -> Result<Self, String> {
        let state_root = ProjectConfig::config_dir()
            .map_err(|e| format!("无法准备同步状态目录：{e}"))?
            .join("sync-state");
        std::fs::create_dir_all(&state_root).map_err(|e| format!("无法准备同步状态目录：{e}"))?;
        Ok(Self::new(
            state_root,
            Arc::new(Secrets::from_env()),
            Arc::new(RemoteDriver::new(env!("CARGO_PKG_VERSION"))),
            true,
        ))
    }

    pub fn new(
        state_root: PathBuf,
        secrets: Arc<Secrets>,
        driver: Arc<dyn SessionDriver>,
        watch: bool,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            state_root,
            secrets,
            driver,
            watch,
        }
    }

    fn project_state_dir(&self, project_id: &str) -> PathBuf {
        self.state_root.join(project_id)
    }

    /// Open (or reuse) the sync session for a saved project.
    pub async fn ensure_open(&self, config: &ProjectConfig) -> Result<(), String> {
        let project_id = config.id.to_string();
        {
            let sessions = self.sessions.lock().map_err(|_| "内部锁失效".to_string())?;
            if sessions.contains_key(&project_id) {
                return Ok(());
            }
        }

        let local_root = PathBuf::from(&config.local_root);
        std::fs::create_dir_all(&local_root).map_err(|e| format!("无法创建本机同步文件夹：{e}"))?;

        let state_dir = self.project_state_dir(&project_id);
        std::fs::create_dir_all(&state_dir).map_err(|e| format!("无法准备项目同步状态：{e}"))?;

        let workspace_id = WorkspaceId::parse(&config.workspace_id.to_string())
            .map_err(|_| "项目 workspace_id 无效".to_string())?;
        let client_id = load_or_create_client_id(&state_dir)?;

        // Includes/excludes come from the project; secret protection is forced
        // inside SyncEngine::open and again in the watcher coalescer below.
        let engine_config = SyncEngineConfig::new(workspace_id, client_id, &local_root, &state_dir)
            .with_sync_rules(config.sync.includes.clone(), config.sync.excludes.clone());
        let engine =
            SyncEngine::open(engine_config).map_err(|e| format!("无法打开同步引擎：{e}"))?;
        let (worker, handle) = EngineWorker::spawn(engine);

        let snapshot = Arc::new(Mutex::new(SyncSnapshot::default()));
        let shutdown = CancellationToken::new();
        let credentials: Arc<dyn CredentialSource> = Arc::new(ProjectAgentCredentials {
            project_id: project_id.clone(),
            state_dir: state_dir.clone(),
            secrets: Arc::clone(&self.secrets),
        });

        let watcher = if self.watch {
            start_watcher(
                &local_root,
                &config.sync.includes,
                &config.sync.excludes,
                // Defense in depth: even a corrupted projects.json cannot turn this off.
                true,
                handle.clone(),
                Arc::clone(&snapshot),
            )
        } else {
            None
        };

        let context = SessionContext {
            workspace_id,
            client_id,
            engine: handle.clone(),
            shutdown: shutdown.clone(),
            credentials,
            snapshot: Arc::clone(&snapshot),
        };
        let driver = Arc::clone(&self.driver);
        let supervisor = tokio::spawn(async move {
            supervise(context, driver).await;
        });

        let mut session = Some(ProjectSession {
            snapshot,
            shutdown,
            supervisor: Some(supervisor),
            engine: handle,
            _worker: worker,
            _watcher: watcher,
        });

        {
            let mut sessions = self.sessions.lock().map_err(|_| "内部锁失效".to_string())?;
            if !sessions.contains_key(&project_id)
                && let Some(ready) = session.take()
            {
                sessions.insert(project_id, ready);
            }
        }
        if let Some(mut duplicate) = session {
            // Another caller won the race; shut this duplicate down outside the lock.
            duplicate.shutdown.cancel();
            if let Some(task) = duplicate.supervisor.take() {
                let _ = task.await;
            }
        }
        Ok(())
    }

    /// Stop the session for a project (e.g. on delete). Missing sessions are fine.
    pub async fn close(&self, project_id: &str) -> Result<(), String> {
        let session = {
            let mut sessions = self.sessions.lock().map_err(|_| "内部锁失效".to_string())?;
            sessions.remove(project_id)
        };
        let Some(mut session) = session else {
            return Ok(());
        };
        // Stop intake before asking the supervisor to shut the engine down.
        drop(session._watcher.take());
        session.shutdown.cancel();
        if let Some(task) = session.supervisor.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .map_err(|_| "关闭同步会话超时".to_string())?;
        }
        Ok(())
    }

    /// 6.3 status for a project. Opens the session lazily when the project exists.
    pub async fn status(&self, project_id: &str) -> Result<SyncStatus, String> {
        if let Some(config) =
            ProjectConfig::get(project_id).map_err(|e| format!("无法读取项目：{e}"))?
        {
            self.ensure_open(&config).await?;
        }
        let Some(handles) = self.session_handles(project_id)? else {
            return Ok(SyncStatus {
                state: SyncState::Offline,
                conflicts: 0,
                pending: 0,
                retry_in_seconds: None,
                detail: Some(DETAIL_NO_ENDPOINT),
            });
        };
        driver::refresh_progress(&handles.engine, &handles.snapshot).await;
        let view = handles
            .snapshot
            .lock()
            .map_err(|_| "内部锁失效".to_string())?
            .clone();
        Ok(view.status(tokio::time::Instant::now()))
    }

    /// Record explicit explorer mutations (create / rename / delete).
    ///
    /// The platform watcher usually sees the same change; the engine deduplicates.
    /// Secret paths are filtered inside the engine (`protect_secrets: true`).
    pub async fn record_paths(
        &self,
        project_id: &str,
        changes: Vec<FsChange>,
    ) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        let Some(handles) = self.session_handles(project_id)? else {
            return Ok(());
        };
        handles
            .engine
            .record_local_changes(changes)
            .await
            .map_err(|e| format!("无法记录本地变更：{e}"))?;
        driver::refresh_progress(&handles.engine, &handles.snapshot).await;
        Ok(())
    }

    /// Project open engine conflicts into the UI shape.
    pub async fn list_engine_conflicts(
        &self,
        project_id: &str,
    ) -> Result<Option<Vec<Conflict>>, String> {
        let Some(handles) = self.session_handles(project_id)? else {
            return Ok(None);
        };
        let conflicts = handles
            .engine
            .with_engine(|engine| project_conflicts(engine))
            .await
            .map_err(|e| format!("无法读取冲突：{e}"))?
            .map_err(|e| format!("无法读取冲突：{e}"))?;
        Ok(Some(conflicts))
    }

    /// Tell the engine about a user resolution. Returns `false` when no session
    /// is open (caller still applies the local ConflictStore write).
    pub async fn resolve_engine_conflict(
        &self,
        project_id: &str,
        conflict_id: &str,
        resolution: Resolution,
    ) -> Result<bool, String> {
        let id = ConflictId::parse(conflict_id).map_err(|_| "冲突编号无效".to_string())?;
        let choice = match resolution {
            Resolution::KeepLocal => WorkspaceConflictChoice::Current,
            Resolution::KeepRemote => WorkspaceConflictChoice::Incoming,
            // Local path keeps the user's version; the sibling copy is desktop-only.
            Resolution::KeepBoth => WorkspaceConflictChoice::Current,
        };
        let Some(handles) = self.session_handles(project_id)? else {
            return Ok(false);
        };
        handles
            .engine
            .with_engine(move |engine| engine.resolve_conflict(id, choice))
            .await
            .map_err(|e| format!("无法提交冲突解决：{e}"))?
            .map_err(|e| format!("无法提交冲突解决：{e}"))?;
        driver::refresh_progress(&handles.engine, &handles.snapshot).await;
        Ok(true)
    }

    fn session_handles(&self, project_id: &str) -> Result<Option<SessionHandles>, String> {
        let sessions = self.sessions.lock().map_err(|_| "内部锁失效".to_string())?;
        Ok(sessions.get(project_id).map(|session| SessionHandles {
            engine: session.engine.clone(),
            snapshot: Arc::clone(&session.snapshot),
        }))
    }
}

struct SessionHandles {
    engine: EngineHandle,
    snapshot: Arc<Mutex<SyncSnapshot>>,
}

/// Endpoint + bearer token for the loopback agent of one project.
struct ProjectAgentCredentials {
    project_id: String,
    state_dir: PathBuf,
    secrets: Arc<Secrets>,
}

impl CredentialSource for ProjectAgentCredentials {
    fn endpoint(&self) -> Option<WorkspaceEndpoint> {
        if let Ok(value) = std::env::var("CCHAVEN_SYNC_ENDPOINT")
            && let Ok(endpoint) = WorkspaceEndpoint::parse(value.trim())
        {
            return Some(endpoint);
        }
        let path = self.state_dir.join("endpoint");
        let text = std::fs::read_to_string(path).ok()?;
        WorkspaceEndpoint::parse(text.trim()).ok()
    }

    fn token(&self) -> Option<SecretToken> {
        if let Ok(value) = std::env::var("CCHAVEN_SYNC_TOKEN") {
            return SecretToken::from_protected_store(value.as_bytes()).ok();
        }
        let secret = self
            .secrets
            .sync_agent_token(&self.project_id)
            .ok()
            .flatten()?;
        SecretToken::from_protected_store(secret.as_bytes()).ok()
    }
}

fn load_or_create_client_id(state_dir: &Path) -> Result<ClientId, String> {
    let path = state_dir.join("client_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if let Ok(id) = ClientId::parse(trimmed) {
            return Ok(id);
        }
    }
    let id = ClientId::parse(&uuid::Uuid::new_v4().to_string())
        .map_err(|_| "无法生成 client_id".to_string())?;
    let mut file = std::fs::File::create(&path).map_err(|e| format!("无法写入 client_id：{e}"))?;
    file.write_all(id.to_string().as_bytes())
        .map_err(|e| format!("无法写入 client_id：{e}"))?;
    Ok(id)
}

fn start_watcher(
    workspace_root: &Path,
    includes: &[String],
    excludes: &[String],
    protect_secrets: bool,
    handle: EngineHandle,
    snapshot: Arc<Mutex<SyncSnapshot>>,
) -> Option<PlatformWatcher> {
    let rules = SyncRules::compile(SyncRuleConfig {
        includes: includes.to_vec(),
        excludes: excludes.to_vec(),
        protect_secrets,
    })
    .ok()?;
    let root = RootedWorkspace::open(workspace_root).ok()?;
    let (watcher, receiver) = start_platform_watcher(&root, 4096).ok()?;
    let debounce = Duration::from_millis(200);

    std::thread::Builder::new()
        .name("cchaven-watch-bridge".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async move {
                let mut coalescer =
                    EventCoalescer::with_rules(debounce, Duration::from_millis(500), 8192, rules);
                loop {
                    match receiver.recv() {
                        Ok(WatchMessage::Event(event)) => {
                            let _ = coalescer.push(event);
                            let now = std::time::Instant::now();
                            match coalescer.flush_ready(now, &ConservativePrior) {
                                Ok(changes) if !changes.is_empty() => {
                                    if handle.record_local_changes(changes).await.is_err() {
                                        break;
                                    }
                                    driver::refresh_progress(&handle, &snapshot).await;
                                }
                                Ok(_) => {}
                                Err(_) => {
                                    let _ = handle
                                        .record_local_changes(vec![FsChange::RescanRequired])
                                        .await;
                                }
                            }
                        }
                        Ok(WatchMessage::Gap(_)) => {
                            let _ = handle
                                .record_local_changes(vec![FsChange::RescanRequired])
                                .await;
                        }
                        Err(_) => break,
                    }
                }
            });
        })
        .ok()?;

    Some(watcher)
}

struct ConservativePrior;

impl PriorEntryLookup for ConservativePrior {
    fn signature(&self, _path: &WorkspacePath) -> Option<fns_fs::EntrySignature> {
        None
    }
}

fn project_conflicts(engine: &SyncEngine) -> Result<Vec<Conflict>, fns_sync_core::SyncError> {
    let mut out = Vec::new();
    for record in engine.state().conflicts()? {
        if record.status == ConflictStatus::Resolving {
            continue;
        }
        let created: WorkspaceConflictCreatedMessage = serde_json::from_slice(&record.created_json)
            .map_err(|_| fns_sync_core::SyncError::CorruptState {
                table: "conflicts",
                field: "created_json",
            })?;
        out.push(Conflict {
            id: created.conflict_id.to_string(),
            path: created.path.as_str().to_string(),
            kind: map_conflict_kind(&created),
            kind_label: map_conflict_kind(&created).label().to_string(),
            detected_at_ms: created
                .current
                .metadata
                .modified_at_ms
                .max(created.incoming.metadata.modified_at_ms),
            local: conflict_side(engine, &created.current),
            remote: conflict_side(engine, &created.incoming),
        });
    }
    Ok(out)
}

fn map_conflict_kind(created: &WorkspaceConflictCreatedMessage) -> ConflictKind {
    match created.kind {
        WorkspaceConflictKind::DeleteModify if created.current.tombstone => {
            ConflictKind::LocalDeleted
        }
        WorkspaceConflictKind::DeleteModify => ConflictKind::RemoteDeleted,
        _ => ConflictKind::BothModified,
    }
}

fn conflict_side(engine: &SyncEngine, side: &fns_protocol::WorkspaceConflictSide) -> ConflictSide {
    if side.tombstone {
        return ConflictSide {
            content: String::new(),
            modified_ms: side.metadata.modified_at_ms,
            deleted: true,
        };
    }
    let content = match &side.content_hash {
        RequiredNullable::Value(hash) => read_blob_text(engine, hash),
        RequiredNullable::Null => String::new(),
    };
    ConflictSide {
        content,
        modified_ms: side.metadata.modified_at_ms,
        deleted: false,
    }
}

fn read_blob_text(engine: &SyncEngine, hash: &fns_protocol::WorkspaceContentHash) -> String {
    let Ok(mut file) = engine.runtime().system().open_blob(hash) else {
        return String::new();
    };
    let mut buf = Vec::new();
    if std::io::Read::take(&mut file, files::MAX_PREVIEW_BYTES + 1)
        .read_to_end(&mut buf)
        .is_err()
    {
        return String::new();
    }
    if buf.len() as u64 > files::MAX_PREVIEW_BYTES {
        return String::new();
    }
    if buf.contains(&0) {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

/// Build an `FsChange` list from explorer paths (root-relative).
pub fn changes_for_create(path: &str) -> Result<Vec<FsChange>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("路径无效：{e}"))?;
    Ok(vec![FsChange::Create(parsed)])
}

pub fn changes_for_rename(from: &str, to: &str) -> Result<Vec<FsChange>, String> {
    let from = WorkspacePath::parse(from).map_err(|e| format!("路径无效：{e}"))?;
    let to = WorkspacePath::parse(to).map_err(|e| format!("路径无效：{e}"))?;
    Ok(vec![FsChange::Rename { from, to }])
}

pub fn changes_for_delete(path: &str) -> Result<Vec<FsChange>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("路径无效：{e}"))?;
    Ok(vec![FsChange::Delete(parsed)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::keychain::MemoryStore;
    use crate::project::{SyncConfig, SyncMode};
    use std::future::Future;
    use std::pin::Pin;
    use tempfile::TempDir;

    struct HoldOpenDriver;

    impl SessionDriver for HoldOpenDriver {
        fn run<'a>(&'a self, context: &'a SessionContext) -> driver::BoxFuture<'a, SessionOutcome> {
            Box::pin(async move {
                context.mark_connected().await;
                context.shutdown.cancelled().await;
                SessionOutcome::Ended(DETAIL_CLOSED)
            })
        }
    }

    struct ScriptedDriver {
        outcomes: Mutex<std::collections::VecDeque<SessionOutcome>>,
    }

    impl SessionDriver for ScriptedDriver {
        fn run<'a>(
            &'a self,
            _context: &'a SessionContext,
        ) -> driver::BoxFuture<'a, SessionOutcome> {
            Box::pin(async move {
                let mut guard = self.outcomes.lock().expect("lock");
                guard
                    .pop_front()
                    .unwrap_or(SessionOutcome::Fatal(DETAIL_PROTOCOL))
            })
        }
    }

    fn test_secrets() -> Arc<Secrets> {
        Arc::new(Secrets::new(Box::new(MemoryStore::new())))
    }

    fn sample_project(_root: &Path, local: &Path) -> ProjectConfig {
        // Persist under a temp config dir by writing projects.json ourselves is
        // heavy; ensure_open only needs the in-memory config for open, while
        // status() may reload from disk. Tests call ensure_open directly.
        ProjectConfig {
            id: uuid::Uuid::new_v4(),
            name: "demo".into(),
            server: crate::project::ServerConfig {
                host: "127.0.0.1".into(),
                port: 22,
                user: "u".into(),
                auth: crate::project::AuthMethod::Password,
                key_path: None,
                config_alias: None,
            },
            remote_root: "/home/u/demo".into(),
            local_root: local.display().to_string(),
            workspace_id: uuid::Uuid::new_v4(),
            tmux_session: "cchaven-demo".into(),
            sync: SyncConfig {
                mode: SyncMode::TwoWaySafe,
                includes: vec!["**".into()],
                excludes: vec![".git/".into()],
                protect_secrets: true,
            },
            created_at: "0".into(),
        }
    }

    #[tokio::test]
    async fn open_close_lifecycle_keeps_a_live_engine() {
        let tmp = TempDir::new().expect("tmp");
        let local = tmp.path().join("workspace");
        std::fs::create_dir_all(&local).expect("local");
        let state = tmp.path().join("state");
        let project = sample_project(tmp.path(), &local);
        let id = project.id.to_string();

        let manager = SyncManager::new(state, test_secrets(), Arc::new(HoldOpenDriver), false);
        manager.ensure_open(&project).await.expect("open");
        manager.ensure_open(&project).await.expect("idempotent");

        // current_thread tests only poll the supervisor when we await.
        let mut synced = false;
        for _ in 0..50 {
            tokio::task::yield_now().await;
            let status = {
                let sessions = manager.sessions.lock().expect("lock");
                sessions
                    .get(&id)
                    .expect("session")
                    .snapshot
                    .lock()
                    .expect("snap")
                    .status(tokio::time::Instant::now())
            };
            if status.state == SyncState::Synced {
                synced = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(synced, "supervisor should mark the session connected");

        manager.close(&id).await.expect("close");
        assert!(manager.sessions.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn unreachable_sessions_climb_the_real_backoff_ladder() {
        let tmp = TempDir::new().expect("tmp");
        let local = tmp.path().join("workspace");
        std::fs::create_dir_all(&local).expect("local");
        let state = tmp.path().join("state");
        let project = sample_project(tmp.path(), &local);

        let driver = Arc::new(ScriptedDriver {
            outcomes: Mutex::new(
                [
                    SessionOutcome::Unreachable(DETAIL_NO_ENDPOINT),
                    SessionOutcome::Unreachable(DETAIL_NO_ENDPOINT),
                    SessionOutcome::Fatal(DETAIL_PROTOCOL),
                ]
                .into_iter()
                .collect(),
            ),
        });
        let manager = SyncManager::new(state, test_secrets(), driver, false);
        manager.ensure_open(&project).await.expect("open");

        // First unreachable → publish retry_at = now+2s.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let status = {
            let sessions = manager.sessions.lock().expect("lock");
            sessions
                .get(&project.id.to_string())
                .expect("session")
                .snapshot
                .lock()
                .expect("snap")
                .status(tokio::time::Instant::now())
        };
        assert_eq!(status.state, SyncState::Offline);
        assert_eq!(status.retry_in_seconds, Some(2));
        assert_eq!(status.detail, Some(DETAIL_NO_ENDPOINT));

        manager.close(&project.id.to_string()).await.expect("close");
    }

    #[tokio::test]
    async fn file_ops_go_through_the_engine_outbox_and_secrets_stay_out() {
        let tmp = TempDir::new().expect("tmp");
        let local = tmp.path().join("workspace");
        std::fs::create_dir_all(&local).expect("local");
        let state = tmp.path().join("state");
        let project = sample_project(tmp.path(), &local);
        let manager = SyncManager::new(state, test_secrets(), Arc::new(HoldOpenDriver), false);
        manager.ensure_open(&project).await.expect("open");
        let id = project.id.to_string();

        std::fs::write(local.join("notes.md"), b"hi").expect("write");
        manager
            .record_paths(&id, changes_for_create("notes.md").expect("path"))
            .await
            .expect("record");

        std::fs::write(local.join(".env"), b"SECRET=1").expect("env");
        manager
            .record_paths(&id, changes_for_create(".env").expect("path"))
            .await
            .expect("record secret");

        let engine = {
            let sessions = manager.sessions.lock().expect("lock");
            sessions.get(&id).expect("session").engine.clone()
        };
        let pending = engine
            .with_engine(|engine| {
                engine
                    .outbox()
                    .expect("outbox")
                    .into_iter()
                    .filter_map(|record| record.mutation().ok())
                    .map(|m| m.path.as_str().to_string())
                    .collect::<Vec<_>>()
            })
            .await
            .expect("with_engine");

        assert!(
            pending.iter().any(|p| p == "notes.md"),
            "expected notes.md in outbox, got {pending:?}"
        );
        assert!(
            !pending.iter().any(|p| p == ".env"),
            "protect_secrets must keep .env out of the outbox, got {pending:?}"
        );

        manager.close(&id).await.expect("close");
    }

    #[tokio::test]
    async fn protect_secrets_cannot_be_turned_off_via_project_config() {
        let tmp = TempDir::new().expect("tmp");
        let local = tmp.path().join("workspace");
        std::fs::create_dir_all(&local).expect("local");
        let state = tmp.path().join("state");
        let mut project = sample_project(tmp.path(), &local);
        project.sync.protect_secrets = false; // ignored by engine + watcher start
        let manager = SyncManager::new(state, test_secrets(), Arc::new(HoldOpenDriver), false);
        manager.ensure_open(&project).await.expect("open");
        let id = project.id.to_string();

        std::fs::write(local.join(".env").as_path(), b"x=1").expect("env");
        manager
            .record_paths(&id, changes_for_create(".env").expect("path"))
            .await
            .expect("record");

        let engine = {
            let sessions = manager.sessions.lock().expect("lock");
            sessions.get(&id).expect("session").engine.clone()
        };
        let pending = engine
            .with_engine(|engine| engine.outbox().expect("outbox").len())
            .await
            .expect("with_engine");
        assert_eq!(pending, 0, "engine must still exclude secrets");
        manager.close(&id).await.expect("close");
    }

    /// Compile-time reminder: SessionDriver is object-safe for Arc injection.
    #[allow(dead_code)]
    fn _driver_object_safe(d: Arc<dyn SessionDriver>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let _ = d;
        })
    }
}
