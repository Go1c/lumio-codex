use codex_plus_core::install::{
    InstallOptions, MANAGER_BUNDLE_ID, SILENT_BINARY, SILENT_BUNDLE_ID, app_bundle_names,
    build_macos_app_bundle, build_windows_entrypoint_plan, companion_binary_path_from_exe,
    default_install_root_strategy, macos_companion_bundle_identifier_from_exe, shortcut_names,
};

#[test]
fn windows_entrypoint_plan_contains_silent_and_manager_entrypoints() {
    let options = InstallOptions {
        install_root: Some("C:/Users/A/Desktop".into()),
        launcher_path: Some("C:/Tools/codex-plus-plus.exe".into()),
        manager_path: Some("C:/Tools/codex-plus-plus-manager.exe".into()),
        remove_owned_data: false,
    };

    let plan = build_windows_entrypoint_plan(&options);

    assert!(plan.silent_shortcut.ends_with("Codex++.lnk"));
    assert!(plan.manager_shortcut.ends_with("Codex++ 管理工具.lnk"));
    assert_eq!(plan.launcher_path, "C:/Tools/codex-plus-plus.exe");
    assert_eq!(plan.manager_path, "C:/Tools/codex-plus-plus-manager.exe");
    assert_eq!(plan.silent_icon_path, "C:/Tools/codex-plus-plus.exe");
    assert_eq!(
        plan.manager_icon_path,
        "C:/Tools/codex-plus-plus-manager.exe"
    );
    assert_eq!(plan.uninstall_key, "CodexPlusPlus");
    assert_eq!(plan.legacy_uninstall_key, "Codex++");
    assert_eq!(
        plan.uninstaller_path.replace('\\', "/"),
        "C:/Tools/uninstall.exe"
    );
    assert_eq!(
        plan.uninstall_command.replace('\\', "/"),
        "\"C:/Tools/uninstall.exe\""
    );
    assert_eq!(
        plan.quiet_uninstall_command.replace('\\', "/"),
        "\"C:/Tools/uninstall.exe\" /S"
    );
    assert_ne!(
        plan.uninstall_command,
        "\"C:/Tools/codex-plus-plus-manager.exe\""
    );
}

#[test]
fn windows_entrypoint_plan_can_request_owned_data_removal_without_shell_script() {
    let options = InstallOptions {
        install_root: Some("C:/Users/A/Desktop".into()),
        launcher_path: None,
        manager_path: None,
        remove_owned_data: true,
    };

    let plan = build_windows_entrypoint_plan(&options);

    assert!(plan.silent_shortcut.ends_with("Codex++.lnk"));
    assert!(plan.manager_shortcut.ends_with("Codex++ 管理工具.lnk"));
    assert!(plan.remove_owned_data);
}

#[test]
fn macos_bundle_metadata_contains_silent_and_manager_apps() {
    let options = InstallOptions {
        install_root: Some("/Applications".into()),
        launcher_path: Some("/opt/Codex++/codex-plus-plus".into()),
        manager_path: Some("/opt/Codex++/codex-plus-plus-manager".into()),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);
    let manager = build_macos_app_bundle(&options, true);

    assert!(silent.app_path.ends_with("Codex++.app"));
    assert!(manager.app_path.ends_with("Codex++ 管理工具.app"));
    assert!(silent.info_plist.contains("<string>Codex++</string>"));
    assert!(
        manager
            .info_plist
            .contains("<string>Codex++ 管理工具</string>")
    );
    assert!(manager.info_plist.contains("<string>dreamskin</string>"));
    assert!(
        manager
            .info_plist
            .contains("<string>codexplusplus</string>")
    );
    assert!(!silent.info_plist.contains("<string>dreamskin</string>"));
    assert_eq!(
        silent.binary_target_name.as_deref(),
        Some("codex-plus-plus")
    );
    assert_eq!(
        manager.binary_target_name.as_deref(),
        Some("codex-plus-plus-manager")
    );
    assert!(silent.launch_script.contains("$DIR/codex-plus-plus"));
    assert!(
        manager
            .launch_script
            .contains("$DIR/codex-plus-plus-manager")
    );
}

#[test]
fn installer_exports_expected_two_entrypoint_names() {
    assert_eq!(shortcut_names(), ("Codex++.lnk", "Codex++ 管理工具.lnk"));
    assert_eq!(app_bundle_names(), ("Codex++.app", "Codex++ 管理工具.app"));
}

#[test]
fn macos_dmg_includes_applications_shortcut_for_drag_install() {
    let script = std::fs::read_to_string("../../scripts/installer/macos/package-dmg.sh")
        .expect("read macOS DMG packaging script");

    assert!(script.contains("ln -s /Applications \"$STAGE/Applications\""));
}

#[test]
fn companion_binary_path_resolves_macos_silent_app_next_to_manager_app() {
    let manager_exe = std::path::Path::new(
        "/Applications/Codex++ 管理工具.app/Contents/MacOS/CodexPlusPlusManager",
    );

    let companion = companion_binary_path_from_exe(manager_exe, SILENT_BINARY);

    assert_eq!(
        companion,
        std::path::PathBuf::from("/Applications/Codex++.app/Contents/MacOS/CodexPlusPlus")
    );
    assert_ne!(
        companion,
        std::path::PathBuf::from(
            "/Applications/Codex++ 管理工具.app/Contents/MacOS/codex-plus-plus"
        )
    );
}

#[test]
fn companion_binary_path_resolves_macos_manager_app_next_to_silent_app() {
    let silent_exe = std::path::Path::new("/Applications/Codex++.app/Contents/MacOS/CodexPlusPlus");

    let companion =
        companion_binary_path_from_exe(silent_exe, codex_plus_core::install::MANAGER_BINARY);

    assert_eq!(
        companion,
        std::path::PathBuf::from(
            "/Applications/Codex++ 管理工具.app/Contents/MacOS/CodexPlusPlusManager"
        )
    );
}

#[test]
fn macos_companion_launch_uses_bundle_ids_from_app_translocation() {
    let manager_exe = std::path::Path::new(
        "/private/var/folders/x/AppTranslocation/manager-id/d/Codex++ 管理工具.app/Contents/MacOS/CodexPlusPlusManager",
    );
    let silent_exe = std::path::Path::new(
        "/private/var/folders/x/AppTranslocation/silent-id/d/Codex++.app/Contents/MacOS/CodexPlusPlus",
    );

    assert_eq!(
        macos_companion_bundle_identifier_from_exe(manager_exe, SILENT_BINARY),
        Some(SILENT_BUNDLE_ID)
    );
    assert_eq!(
        macos_companion_bundle_identifier_from_exe(
            silent_exe,
            codex_plus_core::install::MANAGER_BINARY,
        ),
        Some(MANAGER_BUNDLE_ID)
    );
}

#[test]
fn macos_companion_launch_keeps_bare_binary_development_mode() {
    let manager_exe = std::path::Path::new("/tmp/target/debug/codex-plus-plus-manager");

    assert_eq!(
        macos_companion_bundle_identifier_from_exe(manager_exe, SILENT_BINARY),
        None
    );
}

#[test]
fn macos_bundle_does_not_wrap_the_bundle_executable_in_itself() {
    let options = InstallOptions {
        install_root: Some("/Applications".into()),
        launcher_path: Some("/Applications/Codex++.app/Contents/MacOS/CodexPlusPlus".into()),
        manager_path: Some(
            "/Applications/Codex++ 管理工具.app/Contents/MacOS/CodexPlusPlusManager".into(),
        ),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);
    let manager = build_macos_app_bundle(&options, true);

    assert_eq!(
        silent.binary_source,
        Some(std::path::PathBuf::from(
            "/Applications/Codex++.app/Contents/MacOS/CodexPlusPlus"
        ))
    );
    assert_eq!(
        manager.binary_source,
        Some(std::path::PathBuf::from(
            "/Applications/Codex++ 管理工具.app/Contents/MacOS/CodexPlusPlusManager"
        ))
    );
    assert!(silent.launch_script.contains("$DIR/codex-plus-plus"));
    assert!(
        manager
            .launch_script
            .contains("$DIR/codex-plus-plus-manager")
    );
}

#[test]
fn windows_default_install_root_uses_known_folder_before_userprofile_desktop() {
    let strategy = default_install_root_strategy();

    if cfg!(windows) {
        assert_eq!(strategy, "windows-known-folder");
    } else if cfg!(target_os = "macos") {
        assert_eq!(strategy, "macos-applications");
    } else {
        assert_eq!(strategy, "user-dirs-desktop");
    }
}

fn repository_root() -> std::path::PathBuf {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root");
    repository.to_path_buf()
}

#[test]
fn lumio_desktop_metadata_uses_branded_contract() {
    let repository = repository_root();
    let workspace =
        std::fs::read_to_string(repository.join("Cargo.toml")).expect("read workspace manifest");
    let package_json =
        std::fs::read_to_string(repository.join("apps/codex-plus-manager/package.json"))
            .expect("read frontend package manifest");
    let package_lock =
        std::fs::read_to_string(repository.join("apps/codex-plus-manager/package-lock.json"))
            .expect("read frontend package lock");
    let manager_manifest =
        std::fs::read_to_string(repository.join("apps/codex-plus-manager/src-tauri/Cargo.toml"))
            .expect("read manager manifest");
    let launcher_manifest =
        std::fs::read_to_string(repository.join("apps/codex-plus-launcher/Cargo.toml"))
            .expect("read launcher manifest");
    let tauri_config = std::fs::read_to_string(
        repository.join("apps/codex-plus-manager/src-tauri/tauri.conf.json"),
    )
    .expect("read Tauri config");

    assert!(workspace.contains("https://github.com/LumioGames/lumio-codex"));
    assert!(package_json.contains("\"name\": \"lumio-codex\""));
    assert!(package_lock.contains("\"name\": \"lumio-codex\""));
    assert!(manager_manifest.contains("name = \"lumio-codex\""));
    assert!(launcher_manifest.contains("name = \"lumio-codex-launcher\""));
    for expected in [
        "\"productName\": \"Lumio Codex\"",
        "\"identifier\": \"games.lumio.codex\"",
        "\"title\": \"Lumio Codex\"",
        "icons/icon.icns",
        "icons/icon.ico",
    ] {
        assert!(
            tauri_config.contains(expected),
            "missing metadata: {expected}"
        );
    }
}

#[test]
fn lumio_windows_installer_is_branded_current_user_and_internal_unsigned() {
    let repository = repository_root();
    let windows_installer =
        std::fs::read_to_string(repository.join("scripts/installer/windows/LumioCodex.nsi"))
            .expect("read Lumio Windows installer");

    assert!(windows_installer.contains("Name \"Lumio Codex\""));
    assert!(windows_installer.contains("!ifndef OUT_SUFFIX"));
    assert!(windows_installer.contains("!define OUT_SUFFIX \"-internal-unsigned\""));
    assert!(windows_installer.contains("LumioCodex-${VERSION}-windows-x64-setup${OUT_SUFFIX}.exe"));
    assert!(windows_installer.contains("VIProductVersion"));
    assert!(windows_installer.contains("ProductName"));
    assert!(windows_installer.contains("Lumio Codex"));
    assert!(windows_installer.contains("InstallDir \"$LOCALAPPDATA\\Programs\\Lumio Codex\""));
    assert!(windows_installer.contains("RequestExecutionLevel user"));
    assert!(windows_installer.contains("lumio-codex.exe"));
    assert!(windows_installer.contains("Helpers\\lumio-codex-launcher.exe"));
    assert!(windows_installer.contains("Publisher\" \"Lumio\""));
    assert_eq!(
        windows_installer
            .matches("CreateShortcut \"$DESKTOP")
            .count(),
        1
    );

    for legacy in ["CodexPlusPlus-${VERSION}", "Codex++", "com.bigpizzav3"] {
        assert!(
            !windows_installer.contains(legacy),
            "legacy branding: {legacy}"
        );
    }
}

#[test]
fn lumio_macos_packager_creates_one_visible_internal_unsigned_app() {
    let repository = repository_root();
    let macos_installer =
        std::fs::read_to_string(repository.join("scripts/installer/macos/package-dmg.sh"))
            .expect("read Lumio macOS packager");

    assert!(macos_installer.contains("Lumio Codex.app"));
    assert!(macos_installer.contains("games.lumio.codex"));
    assert!(macos_installer.contains("LumioCodex-${VERSION}-macos-${ARCH}-internal-unsigned.dmg"));
    assert!(macos_installer.contains("Contents/Helpers/lumio-codex-launcher"));
    assert!(macos_installer.contains("$BINARY_DIR/lumio-codex"));
    assert!(macos_installer.contains("$BINARY_DIR/lumio-codex-launcher"));
    assert!(macos_installer.contains("mktemp -d"));
    assert!(!macos_installer.contains("rm -rf"));
    assert!(!macos_installer.contains("CFBundleURLTypes"));
    assert!(!macos_installer.contains("codesign"));

    for legacy in ["CodexPlusPlus-${VERSION}", "com.bigpizzav3.codexplusplus"] {
        assert!(!macos_installer.contains(legacy));
    }
}
