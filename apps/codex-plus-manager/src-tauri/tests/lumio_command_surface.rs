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
            "lumio_commands::lumio_public_settings",
            "lumio_commands::lumio_send_verify_code",
            "lumio_commands::lumio_register",
            "lumio_commands::lumio_login",
            "lumio_commands::lumio_login_two_factor",
            "lumio_commands::lumio_logout",
            "lumio_commands::lumio_refresh_account",
            "lumio_commands::lumio_provision_step",
            "lumio_commands::lumio_takeover_health",
            "lumio_commands::lumio_restore_config",
            "lumio_commands::lumio_launch_codex",
            "lumio_commands::lumio_detect_codex_app",
            "lumio_commands::lumio_select_codex_app",
            "lumio_commands::lumio_open_browser",
            "lumio_commands::lumio_check_update",
            "lumio_commands::lumio_set_telemetry",
            "lumio_commands::lumio_export_logs",
            "lumio_hide_to_tray",
            "lumio_exit_app",
        ]
    );
}

#[test]
fn every_registered_command_uses_the_lumio_prefix() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs");
    let handler = source
        .split_once(".invoke_handler(tauri::generate_handler![")
        .and_then(|(_, rest)| rest.split_once("])"))
        .map(|(handler, _)| handler)
        .expect("Lumio invoke handler");

    for line in handler
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let command = line.trim_end_matches(',').rsplit("::").next().unwrap();
        assert!(
            command.starts_with("lumio_"),
            "command outside the lumio surface: {command}"
        );
    }
}

#[test]
fn command_payloads_never_expose_tokens_or_key_material() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");

    let payload_structs = source
        .split("#[derive(Debug, Serialize)]")
        .skip(1)
        .collect::<Vec<_>>();
    assert!(
        !payload_structs.is_empty(),
        "no serializable payloads found"
    );

    for block in payload_structs {
        let body = block.split_once('}').map(|(head, _)| head).unwrap_or(block);
        for forbidden in ["access_token", "refresh_token", "temp_token", "api_key"] {
            assert!(
                !body.contains(forbidden),
                "serialized payload leaks {forbidden}:\n{body}"
            );
        }
    }
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
