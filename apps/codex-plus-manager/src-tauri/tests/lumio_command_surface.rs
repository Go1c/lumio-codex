use std::fs;
use std::path::PathBuf;

#[test]
fn lumio_builder_registers_only_the_lumio_allowlist() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs");
    let handler = source
        .split_once(".invoke_handler(tauri::generate_handler![")
        .and_then(|(_, rest)| rest.split_once("])"))
        .map(|(handler, _)| handler)
        .expect("Lumio invoke handler");
    let commands = handler
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(','))
        .collect::<Vec<_>>();

    assert_eq!(
        commands,
        [
            "lumio_commands::lumio_bootstrap",
            "lumio_hide_to_tray",
            "lumio_exit_app",
        ]
    );
}

#[test]
fn lumio_builder_does_not_register_codex_plus_enhancement_commands() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs");

    assert!(source.contains("lumio_commands::lumio_bootstrap"));
    for forbidden in [
        "commands::apply_dream_skin",
        "commands::refresh_script_market",
        "commands::repair_plugin_marketplace",
        "commands::apply_relay_injection",
        "commands::list_local_sessions",
    ] {
        assert!(
            !source.contains(forbidden),
            "registered forbidden command: {forbidden}"
        );
    }
}

#[test]
fn lumio_entrypoint_has_no_legacy_url_or_skin_processing() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(root.join("src/main.rs")).expect("main.rs");

    assert!(main.contains("codex_plus_manager_lib::run();"));
    for forbidden in ["dreamskin://", "codexplusplus://", "provider_import"] {
        assert!(
            !main.contains(forbidden),
            "legacy entrypoint remains: {forbidden}"
        );
    }
}
