#[allow(dead_code)]
mod commands;
#[allow(dead_code)]
mod install;
pub mod lumio_commands;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

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
            let mut main_window_builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title(codex_plus_core::lumio::product::PRODUCT_NAME)
            .inner_size(1040.0, 720.0)
            .min_inner_size(760.0, 620.0);
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
    let show_item = MenuItem::with_id(app, TRAY_MENU_SHOW, "显示 Lumio Codex", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT, "退出 Lumio Codex", true, None::<&str>)?;
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
