mod claude_commands;
mod claude_conflicts;
mod claude_deploy;
mod claude_files;
mod claude_ssh;
mod claude_sync;
mod claude_terminal;
mod claude_tunnel;
#[allow(dead_code)]
mod commands;
#[allow(dead_code)]
mod install;
pub mod lumio_commands;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

const TRAY_ID: &str = "lumio_codex_tray";
const TRAY_MENU_SHOW: &str = "lumio_tray_show";
const TRAY_MENU_QUIT: &str = "lumio_tray_quit";
static APP_EXITING: AtomicBool = AtomicBool::new(false);

pub fn run() {
    install_panic_logger();
    let Some(_guard) = acquire_single_instance_guard() else {
        return;
    };

    let app_result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(lumio_commands::LumioSession::new().map_err(|error| error.to_string())?);
            app.manage(claude_sync::SyncEngine::new());
            app.manage(claude_terminal::TerminalManager::new());
            app.manage(claude_tunnel::TunnelManager::new());
            let mut main_window_builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title(codex_plus_core::lumio::product::PRODUCT_NAME)
            .inner_size(1040.0, 720.0)
            .min_inner_size(760.0, 620.0);
            #[cfg(target_os = "macos")]
            {
                main_window_builder = main_window_builder
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true);
            }
            if let Some(icon) = app.default_window_icon().cloned() {
                main_window_builder = main_window_builder.icon(icon)?;
            }
            let main_window = main_window_builder.build()?;
            install_tray(app)?;
            register_main_window_events(main_window, startup_is_transient());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            lumio_commands::lumio_bootstrap,
            lumio_commands::lumio_public_settings,
            lumio_commands::lumio_send_verify_code,
            lumio_commands::lumio_register,
            lumio_commands::lumio_login,
            lumio_commands::lumio_login_two_factor,
            lumio_commands::lumio_logout,
            lumio_commands::lumio_refresh_account,
            lumio_commands::lumio_claude_entitlement,
            lumio_commands::lumio_provision_step,
            lumio_commands::lumio_takeover_health,
            lumio_commands::lumio_restore_config,
            lumio_commands::lumio_launch_codex,
            lumio_commands::lumio_detect_codex_app,
            lumio_commands::lumio_select_codex_app,
            lumio_commands::lumio_open_browser,
            lumio_commands::lumio_check_update,
            lumio_commands::lumio_download_update,
            lumio_commands::lumio_dismiss_update,
            lumio_commands::lumio_update_notice_shown,
            lumio_commands::lumio_set_telemetry,
            lumio_commands::lumio_set_launch_at_login,
            lumio_commands::lumio_export_logs,
            lumio_commands::lumio_install_official_app,
            lumio_commands::lumio_official_app_status,
            lumio_commands::lumio_cancel_official_app,
            claude_commands::lumio_claude_probe_connection,
            claude_commands::lumio_claude_prepare_remote,
            claude_commands::lumio_claude_first_sync,
            claude_commands::lumio_claude_open_system_terminal,
            claude_commands::lumio_claude_run_remote,
            claude_commands::lumio_claude_list_local_files,
            claude_commands::lumio_claude_list_files,
            claude_commands::lumio_claude_preview_file,
            claude_commands::lumio_claude_list_conflicts,
            claude_commands::lumio_claude_resolve_conflict,
            claude_commands::lumio_claude_conflict_diff,
            claude_commands::lumio_claude_list_ssh_hosts,
            claude_commands::lumio_claude_start_terminal,
            claude_commands::lumio_claude_write_terminal,
            claude_commands::lumio_claude_resize_terminal,
            lumio_hide_to_tray,
            lumio_exit_app,
        ])
        .build(tauri::generate_context!());

    match app_result {
        Ok(app) => app.run(|_, _| {}),
        Err(_) => eprintln!("LUMIO_MANAGER_RUN_FAILED"),
    }
}

fn install_tray<R: tauri::Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, TRAY_MENU_SHOW, "显示 BestCodex", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT, "退出 BestCodex", true, None::<&str>)?;
    let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut tray_builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&tray_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            TRAY_MENU_SHOW => show_main_window(app),
            TRAY_MENU_QUIT => {
                APP_EXITING.store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => show_main_window(&tray.app_handle()),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }

    let _ = tray_builder.build(app)?;
    Ok(())
}

fn register_main_window_events<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    transient: bool,
) {
    let event_window = window.clone();
    let minimized_window = event_window.clone();
    let close_event_window = event_window.clone();
    let close_event_app = event_window.app_handle().clone();

    event_window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) => {
            if matches!(minimized_window.is_minimized(), Ok(true)) {
                let _ = minimized_window.hide();
            }
        }
        WindowEvent::CloseRequested { api, .. } => {
            if APP_EXITING.load(Ordering::SeqCst) {
                return;
            }

            if transient {
                APP_EXITING.store(true, Ordering::SeqCst);
                close_event_app.exit(0);
                return;
            }

            api.prevent_close();
            let _ = close_event_window.hide();
        }
        _ => {}
    });
}

fn startup_is_transient() -> bool {
    std::env::args().any(|arg| arg == "--transient")
}

#[tauri::command]
fn lumio_exit_app<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    APP_EXITING.store(true, Ordering::SeqCst);
    app.exit(0);
}

#[tauri::command]
fn lumio_hide_to_tray<R: tauri::Runtime>(window: tauri::WebviewWindow<R>) {
    let _ = window.hide();
}

fn show_main_window<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        // 托盘唤起是「回来看余额」的典型时机；前端监听后按距上次同步节流拉取
        //（事件名与前端 account-refresh.ts 的 WINDOW_SHOWN_EVENT 逐字一致）。
        let _ = window.emit("lumio://window-shown", ());
    }
}

fn focus_existing_lumio_window() {
    #[cfg(windows)]
    {
        let Some(executable_name) = std::env::current_exe()
            .ok()
            .and_then(|path| path.file_name().map(|name| name.to_os_string()))
        else {
            return;
        };
        let executable_name = executable_name.to_string_lossy();
        let current_process_id = std::process::id();
        for process in codex_plus_core::windows_enumerate_processes() {
            if process.process_id != current_process_id
                && process
                    .exe_file
                    .eq_ignore_ascii_case(executable_name.as_ref())
            {
                let _ = codex_plus_core::windows_activate_process_window(process.process_id);
                break;
            }
        }
    }
}

fn install_panic_logger() {
    std::panic::set_hook(Box::new(|_| {
        eprintln!("LUMIO_MANAGER_PANIC");
    }));
}

fn acquire_single_instance_guard() -> Option<codex_plus_core::ports::LoopbackPortGuard> {
    match codex_plus_core::ports::acquire_resilient_loopback_port_guard(
        codex_plus_core::ports::manager_guard_port(),
    ) {
        Ok(guard) => Some(guard),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::WouldBlock
            ) =>
        {
            focus_existing_lumio_window();
            None
        }
        Err(_) => std::net::TcpListener::bind(("127.0.0.1", 0))
            .ok()
            .map(codex_plus_core::ports::LoopbackPortGuard::listener),
    }
}
