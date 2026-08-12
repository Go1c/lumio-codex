//! CLI subprocess tests for fns-agent.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_fns-agent");

fn write_test_config(dir: &std::path::Path) -> std::path::PathBuf {
    let config_path = dir.join("agent.json");
    let workspace_root = dir.join("workspace");
    let state_dir = dir.join("state");
    let token_file = dir.join("token");
    std::fs::create_dir_all(&workspace_root).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(&token_file, b"test-token-value").unwrap();

    let config = serde_json::json!({
        "schemaVersion": "fns-agent-config/1",
        "endpoint": "ws://127.0.0.1:8080/api/user/workspace-sync/v2",
        "workspaceId": "10000000-0000-4000-8000-000000000002",
        "clientId": "10000000-0000-4000-8000-000000000001",
        "workspaceRoot": workspace_root.to_str().unwrap(),
        "stateDir": state_dir.to_str().unwrap(),
        "tokenFile": token_file.to_str().unwrap(),
        "sync": {
            "includes": ["**"],
            "excludes": [],
            "protectSecrets": true
        },
        "transport": {
            "maxActiveTransfers": 2
        }
    });
    std::fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();

    // On Linux, config and token files must be 0600 for security validation.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    config_path
}

#[test]
fn status_on_stopped_config_emits_json_and_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_test_config(dir.path());

    let output = Command::new(BIN)
        .args([
            "status",
            "--config",
            config_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("failed to parse stdout as JSON: {stdout}"));
    assert_eq!(value["schemaVersion"], "fns-agent-status/1");
    assert_eq!(value["running"], false);
    assert_eq!(value["phase"], "stopped");
    assert!(value["pid"].is_null());
}

#[test]
fn diagnose_emits_json_with_checks() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_test_config(dir.path());

    let output = Command::new(BIN)
        .args([
            "diagnose",
            "--config",
            config_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("failed to parse stdout as JSON: {stdout}"));

    assert_eq!(value["schemaVersion"], "fns-agent-diagnose/1");
    assert!(value["checks"].is_array());
    let checks = value["checks"].as_array().unwrap();
    assert!(checks.len() >= 5);

    // Config file check should pass.
    let config_check = checks.iter().find(|c| c["name"] == "config_file").unwrap();
    assert_eq!(config_check["status"], "pass");
}

#[test]
fn run_without_config_exits_2() {
    let output = Command::new(BIN).args(["run"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn status_without_json_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_test_config(dir.path());
    let output = Command::new(BIN)
        .args(["status", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
