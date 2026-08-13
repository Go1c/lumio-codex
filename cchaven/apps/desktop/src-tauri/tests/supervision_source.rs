#[test]
fn desktop_source_has_no_hardcoded_jwt_or_frontend_token_argument() {
    let source = include_str!("../src/sync.rs");
    assert!(!source.contains("DEFAULT_TOKEN"));
    assert!(!source.contains("token: Option<String>"));
    assert!(!source.contains("from_bytes_for_test"));
    assert!(!source.contains("run_embedded"));
}

#[test]
fn macos_arm64_worker_sidecar_is_declared() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(
        config["bundle"]["externalBin"],
        serde_json::json!(["binaries/fns-agent"])
    );
}

#[test]
fn app_exit_reports_sync_shutdown_errors() {
    let source = include_str!("../src/lib.rs");
    assert!(source.contains("fns_sync_shutdown_failed:"));
    assert!(!source.contains("let _ = tauri::async_runtime::block_on(sync.shutdown_all())"));
}
