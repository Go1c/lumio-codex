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
            "lumio_commands::lumio_claude_entitlement",
            "lumio_commands::lumio_claude_pay_with_balance",
            "lumio_commands::lumio_claude_orders",
            "lumio_commands::lumio_claude_plan",
            "lumio_commands::lumio_provision_step",
            "lumio_commands::lumio_takeover_health",
            "lumio_commands::lumio_restore_config",
            "lumio_commands::lumio_launch_codex",
            "lumio_commands::lumio_detect_codex_app",
            "lumio_commands::lumio_select_codex_app",
            "lumio_commands::lumio_open_browser",
            "lumio_commands::lumio_check_update",
            "lumio_commands::lumio_download_update",
            "lumio_commands::lumio_dismiss_update",
            "lumio_commands::lumio_update_notice_shown",
            "lumio_commands::lumio_set_telemetry",
            "lumio_commands::lumio_set_launch_at_login",
            "lumio_commands::lumio_export_logs",
            "lumio_commands::lumio_install_official_app",
            "lumio_commands::lumio_official_app_status",
            "lumio_commands::lumio_cancel_official_app",
            "claude_commands::lumio_claude_probe_connection",
            "claude_commands::lumio_claude_inspect_remote",
            "claude_commands::lumio_claude_prepare_remote",
            "claude_commands::lumio_claude_first_sync",
            "claude_commands::lumio_claude_open_system_terminal",
            "claude_commands::lumio_claude_run_remote",
            "claude_commands::lumio_claude_list_local_files",
            "claude_commands::lumio_claude_list_files",
            "claude_commands::lumio_claude_preview_file",
            "claude_commands::lumio_claude_local_fs",
            "claude_commands::lumio_claude_list_conflicts",
            "claude_commands::lumio_claude_resolve_conflict",
            "claude_commands::lumio_claude_conflict_diff",
            "claude_commands::lumio_claude_list_ssh_hosts",
            "claude_commands::lumio_claude_start_terminal",
            "claude_commands::lumio_claude_write_terminal",
            "claude_commands::lumio_claude_resize_terminal",
            "claude_commands::lumio_claude_open_chat",
            "claude_commands::lumio_claude_close_chat",
            "claude_commands::lumio_claude_list_chats",
            "claude_commands::lumio_claude_resume_sync",
            "claude_commands::lumio_claude_server_status",
            "claude_commands::lumio_claude_list_sessions",
            "claude_cli::lumio_claude_install_cli",
            "claude_login::lumio_claude_login_start",
            "claude_login::lumio_claude_login_submit",
            "claude_login::lumio_claude_login_status",
            "lumio_hide_to_tray",
            "lumio_exit_app",
        ]
    );
}

#[test]
fn prepare_and_inspect_leave_the_ui_thread() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source =
        fs::read_to_string(root.join("src/claude_commands.rs")).expect("claude_commands.rs");
    for name in [
        "pub async fn lumio_claude_inspect_remote",
        "pub async fn lumio_claude_prepare_remote",
    ] {
        assert!(
            source.contains(name),
            "{name} must be async so SCP/SSH cannot freeze the window"
        );
    }
    let prepare = source
        .split("pub async fn lumio_claude_prepare_remote")
        .nth(1)
        .expect("prepare command");
    assert!(
        prepare.contains("spawn_blocking"),
        "prepare must run SCP off the UI thread"
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

#[test]
fn lumio_register_accepts_the_invitation_code_argument() {
    // 前端 registerAccount 一直发送 invitationCode；命令签名一旦漏掉该参数，
    // Tauri 会静默丢弃它，邀请码模式下注册将陷入死循环（QA D-1）。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");

    let register = source
        .split_once("pub async fn lumio_register(")
        .and_then(|(_, rest)| rest.split_once(") -> Result<LumioCommandResult<LumioAuthPayload>"))
        .map(|(signature, _)| signature)
        .expect("lumio_register signature");
    assert!(
        register.contains("invitation_code: String"),
        "lumio_register must accept invitation_code from the frontend:\n{register}"
    );

    // 透传而非硬编码 None：请求构造必须引用参数。
    let body = source
        .split_once("pub async fn lumio_register(")
        .and_then(|(_, rest)| rest.split_once("pub async fn lumio_login("))
        .map(|(body, _)| body)
        .expect("lumio_register body");
    assert!(
        body.contains("invitation_code: non_empty(invitation_code)"),
        "lumio_register must forward the invitation code:\n{body}"
    );
}

#[test]
fn lumio_settings_payload_carries_the_invitation_switch() {
    // settings 下发缺这个开关，注册页的邀请码输入框就只能等服务端报错后才出现。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");

    let payload = source
        .split_once("pub struct LumioServiceSettingsPayload {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(fields, _)| fields)
        .expect("LumioServiceSettingsPayload fields");
    assert!(
        payload.contains("invitation_code_enabled"),
        "LumioServiceSettingsPayload must expose invitation_code_enabled:\n{payload}"
    );
}

#[test]
fn bootstrap_version_uses_the_ci_package_display_label() {
    // 官网下载卡写 `v1.2.46-internal-95`；页脚若只吐 CARGO_PKG_VERSION，
    // 安装包与站点对不上，长标签还会在右下角被裁切。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");
    let body = source
        .split_once("pub fn lumio_bootstrap(")
        .and_then(|(_, rest)| rest.split_once("#[tauri::command]"))
        .map(|(body, _)| body)
        .expect("lumio_bootstrap body");

    assert!(
        body.contains("resolve_display_version_with_bundle"),
        "bootstrap.version must use the package display label:\n{body}"
    );
    assert!(
        body.contains("option_env!(\"LUMIO_PACKAGE_VERSION\")"),
        "CI must be able to stamp the internal label at compile time:\n{body}"
    );
    assert!(
        body.contains("running_bundle_short_version"),
        "missing CI label must fall back to the installed bundle short version:\n{body}"
    );
}

#[test]
fn lumio_install_official_app_accepts_the_destination_argument() {
    // 前端选择目录后传 destination；命令签名一旦漏掉该参数，Tauri 会静默丢弃，
    // 用户选的目录被无视、仍装到默认位置（D-23，同 D-1 的静默丢参坑）。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");

    let signature = source
        .split_once("pub async fn lumio_install_official_app(")
        .and_then(|(_, rest)| {
            rest.split_once(") -> Result<LumioCommandResult<LumioOfficialAppInstallPayload>")
        })
        .map(|(signature, _)| signature)
        .expect("lumio_install_official_app signature");
    assert!(
        signature.contains("destination: Option<String>"),
        "lumio_install_official_app must accept destination from the frontend:\n{signature}"
    );

    let body = source
        .split_once("pub async fn lumio_install_official_app(")
        .and_then(|(_, rest)| rest.split_once("pub fn lumio_official_app_status"))
        .map(|(body, _)| body)
        .expect("lumio_install_official_app body");
    assert!(
        body.contains("begin_background_install(session_app, destination)"),
        "the destination must be forwarded to the install pipeline:\n{body}"
    );
}

#[test]
fn app_surface_commands_detect_through_the_saved_destination() {
    // 自选目录装的官方应用只有 detect_existing_app 认得（保存路径优先、自动扫描
    // 兜底）。bootstrap / detect / launch 若直连 resolve_codex_app_dir，装完立刻
    // 启动报 CODEX_APP_NOT_FOUND、重启后首页「未检测到官方应用」（QA D-24）。
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/lumio_commands.rs")).expect("lumio_commands.rs");

    for command in [
        "pub fn lumio_bootstrap(",
        "pub fn lumio_detect_codex_app(",
        "pub fn lumio_launch_codex(",
    ] {
        let body = source
            .split_once(command)
            .and_then(|(_, rest)| rest.split_once("#[tauri::command]"))
            .map(|(body, _)| body)
            .unwrap_or_else(|| panic!("{command} body"));
        assert!(
            body.contains("official_app_install::detect_existing_app"),
            "{command} must resolve the app through detect_existing_app:\n{body}"
        );
        assert!(
            !body.contains("app_paths::resolve_codex_app_dir"),
            "{command} bypasses the saved install destination:\n{body}"
        );
    }
}

#[test]
fn sidecar_placeholders_cover_ci_host_triples() {
    // tauri-build resolves externalBin as binaries/<name>-<triple>[.exe].
    // Missing Windows placeholder makes `cargo test` panic in build.rs before
    // any setup/zip is produced.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = fs::read_to_string(root.join("tauri.conf.json")).expect("tauri.conf.json");
    assert!(
        config.contains(r#""externalBin": ["binaries/fns-agent"]"#),
        "update this test if the sidecar name changes:\n{config}"
    );
    let binaries = root.join("binaries");
    for name in [
        "fns-agent-aarch64-apple-darwin",
        "fns-agent-x86_64-apple-darwin",
        "fns-agent-x86_64-pc-windows-msvc.exe",
    ] {
        let path = binaries.join(name);
        assert!(
            path.is_file(),
            "missing sidecar placeholder for cargo test/build: {}",
            path.display()
        );
    }
}
