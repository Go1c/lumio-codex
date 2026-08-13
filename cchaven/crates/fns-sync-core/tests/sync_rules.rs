//! Synchronization rules are enforced by the engine, not by its callers.
//!
//! 交互设计 5.3 promises 「机密文件（.env、密钥）默认受保护，永不同步」 with no way to
//! turn it off in M3. These tests pin that promise to the engine boundary: a
//! host that hands the engine a change for a secret file gets no mutation, and
//! there is no configuration that changes the answer.

use std::fs;

use fns_fs::{FsChange, SyncRuleConfig};
use fns_protocol::{ClientId, WorkspaceId, WorkspacePath};
use fns_sync_core::{SyncEngine, SyncEngineConfig, SyncError};
use tempfile::TempDir;

/// Every secret shape `fns-fs` hard-excludes that a user might plausibly keep
/// inside a project folder.
const SECRETS: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    "config/.env",
    "deploy.pem",
    "certs/server.key",
    "id_rsa",
    "keys/id_ed25519",
    ".ssh/config",
    ".aws/credentials",
    ".kube/config",
    ".docker/config.json",
];

struct Fixture {
    engine: SyncEngine,
    workspace: TempDir,
    _state: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::with_rules(Vec::new(), Vec::new())
    }

    fn with_rules(includes: Vec<String>, excludes: Vec<String>) -> Self {
        let workspace = tempfile::tempdir().expect("workspace directory");
        let state = tempfile::tempdir().expect("state directory");
        let config = SyncEngineConfig::new(
            WorkspaceId::parse("20000000-0000-4000-8000-000000000001").expect("workspace id"),
            ClientId::parse("20000000-0000-4000-8000-000000000002").expect("client id"),
            workspace.path(),
            state.path(),
        )
        .with_sync_rules(SyncRuleConfig {
            includes,
            excludes,
            protect_secrets: true,
        });
        Self {
            engine: SyncEngine::open(config).expect("engine"),
            workspace,
            _state: state,
        }
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.workspace.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent directory");
        }
        fs::write(path, bytes).expect("write");
    }

    /// Hand the engine a change the way a host would, then report the paths it
    /// decided to send to the server.
    fn record(&mut self, change: FsChange) -> Vec<String> {
        self.engine
            .record_local_changes([change])
            .expect("record local change");
        self.engine
            .outbox()
            .expect("outbox")
            .into_iter()
            .map(|record| record.mutation().expect("mutation").path.to_string())
            .collect()
    }
}

fn workspace_path(value: &str) -> WorkspacePath {
    WorkspacePath::parse(value).expect("workspace path")
}

/// Paths of queued *file* mutations. Directories sync as empty containers even
/// when everything inside them is excluded, which is not what these tests are
/// about.
fn queued_files(fixture: &Fixture) -> Vec<String> {
    fixture
        .engine
        .outbox()
        .expect("outbox")
        .into_iter()
        .filter_map(|record| {
            let mutation = record.mutation().expect("mutation");
            (mutation.kind != fns_protocol::WorkspaceMutationKind::Mkdir)
                .then(|| mutation.path.to_string())
        })
        .collect()
}

#[test]
fn secret_files_never_reach_the_outbox_even_when_a_host_asks_directly() {
    for secret in SECRETS {
        let mut fixture = Fixture::new();
        fixture.write(secret, b"SECRET_TOKEN=hunter2\n");

        let queued = fixture.record(FsChange::Create(workspace_path(secret)));

        assert!(
            queued.is_empty(),
            "{secret} was queued for synchronization: {queued:?}"
        );
    }
}

#[test]
fn ordinary_files_beside_a_secret_still_synchronize() {
    let mut fixture = Fixture::new();
    fixture.write(".env", b"TOKEN=1\n");
    fixture.write("src/main.rs", b"fn main() {}\n");

    fixture
        .engine
        .record_local_changes([
            FsChange::Create(workspace_path(".env")),
            FsChange::Create(workspace_path("src/main.rs")),
        ])
        .expect("record");

    let queued = fixture
        .engine
        .outbox()
        .expect("outbox")
        .into_iter()
        .map(|record| record.mutation().expect("mutation").path.to_string())
        .collect::<Vec<_>>();
    assert_eq!(queued, vec!["src/main.rs".to_string()]);
}

#[test]
fn a_full_scan_walks_past_secrets_too() {
    let mut fixture = Fixture::new();
    for secret in SECRETS {
        fixture.write(secret, b"SECRET\n");
    }
    fixture.write("README.md", b"hello\n");

    fixture.engine.scan_and_record().expect("scan and record");

    // The folders holding the secrets are ordinary directories and do sync —
    // empty, because none of their contents are admitted. Only file contents
    // are the promise here.
    assert_eq!(queued_files(&fixture), vec!["README.md".to_string()]);
}

#[test]
fn an_env_template_is_not_a_secret() {
    // `.env.example` is documentation that projects expect to share.
    let mut fixture = Fixture::new();
    fixture.write(".env.example", b"TOKEN=\n");

    let queued = fixture.record(FsChange::Create(workspace_path(".env.example")));

    assert_eq!(queued, vec![".env.example".to_string()]);
}

#[test]
fn no_user_rule_can_re_admit_a_secret() {
    // The most determined attempt available through the config surface: name
    // the secrets as explicit includes and clear every exclude.
    let includes = SECRETS
        .iter()
        .map(|secret| (*secret).to_string())
        .chain(["**".to_string()])
        .collect::<Vec<_>>();
    let mut fixture = Fixture::with_rules(includes, Vec::new());

    for secret in SECRETS {
        fixture.write(secret, b"SECRET\n");
        assert!(
            !fixture
                .engine
                .path_is_synced(&workspace_path(secret))
                .expect("rule decision"),
            "an explicit include re-admitted {secret}"
        );
    }

    fixture.engine.scan_and_record().expect("scan and record");
    assert!(queued_files(&fixture).is_empty());
}

#[test]
fn user_excludes_apply_on_top_of_secret_protection() {
    let mut fixture = Fixture::with_rules(Vec::new(), vec!["notes/**".to_string()]);
    fixture.write("notes/scratch.md", b"draft\n");
    fixture.write("src/lib.rs", b"\n");

    fixture.engine.scan_and_record().expect("scan and record");

    assert_eq!(queued_files(&fixture), vec!["src/lib.rs".to_string()]);
}

#[test]
fn renaming_a_tracked_file_into_a_secret_deletes_it_on_the_server() {
    let mut fixture = Fixture::new();
    fixture.write("settings.txt", b"TOKEN=1\n");
    fixture.engine.scan_and_record().expect("initial scan");
    assert_eq!(fixture.engine.outbox().expect("outbox").len(), 1);

    fs::rename(
        fixture.workspace.path().join("settings.txt"),
        fixture.workspace.path().join(".env"),
    )
    .expect("rename");
    fixture
        .engine
        .record_local_changes([FsChange::Rename {
            from: workspace_path("settings.txt"),
            to: workspace_path(".env"),
        }])
        .expect("record rename");

    let kinds = fixture
        .engine
        .outbox()
        .expect("outbox")
        .into_iter()
        .map(|record| {
            let mutation = record.mutation().expect("mutation");
            (mutation.path.to_string(), mutation.kind)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![(
            "settings.txt".to_string(),
            fns_protocol::WorkspaceMutationKind::Delete
        )],
        "the renamed-away file must not linger on the server, and .env must not appear"
    );
}

#[test]
fn renaming_a_secret_into_a_tracked_name_creates_it() {
    // The user has decided this file is no longer a secret; honouring the
    // rename as a create is what makes the folder self-consistent.
    let mut fixture = Fixture::new();
    fixture.write(".env", b"NOTES=1\n");
    fs::rename(
        fixture.workspace.path().join(".env"),
        fixture.workspace.path().join("settings.txt"),
    )
    .expect("rename");

    let queued = fixture.record(FsChange::Rename {
        from: workspace_path(".env"),
        to: workspace_path("settings.txt"),
    });

    assert_eq!(queued, vec!["settings.txt".to_string()]);
}

#[test]
fn deleting_a_secret_is_not_reported_to_the_server() {
    let mut fixture = Fixture::new();
    fixture.write(".env", b"TOKEN=1\n");
    fs::remove_file(fixture.workspace.path().join(".env")).expect("remove");

    let queued = fixture.record(FsChange::Delete(workspace_path(".env")));

    assert!(queued.is_empty(), "{queued:?}");
}

#[test]
fn a_closed_engine_rejects_local_changes_rather_than_queueing_them() {
    let mut fixture = Fixture::new();
    fixture.write("src/main.rs", b"\n");
    fixture.engine.close().expect("close");

    assert_eq!(
        fixture
            .engine
            .record_local_changes([FsChange::Create(workspace_path("src/main.rs"))]),
        Err(SyncError::ProtocolInvariant {
            reason: "engine_closed"
        })
    );
}
