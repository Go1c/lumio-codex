// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    // OpenSSH re-executes this binary as its `SSH_ASKPASS` helper. Answer and
    // exit before any window is created.
    if let Ok(socket) = std::env::var(fns_workspace_desktop_lib::askpass::SOCKET_ENV) {
        return fns_workspace_desktop_lib::askpass::run_askpass_helper(&socket);
    }

    fns_workspace_desktop_lib::run();
    ExitCode::SUCCESS
}
