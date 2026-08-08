//! FNS Workspace Desktop — Tauri 2 backend.
//!
//! Provides project configuration, SSH host parsing, and deployment orchestration
//! commands for the macOS desktop application.

mod files;
mod project;
mod ssh;
mod ssh_tunnel;
mod sync;
mod terminal;

use project::ProjectConfig;
use serde::{Deserialize, Serialize};

/// Learn Tauri command — returns a greeting (placeholder for onboarding).
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! Welcome to FNS Workspace.")
}

/// Save a project configuration.
#[tauri::command]
fn save_project(config: ProjectConfig) -> Result<String, String> {
    let id = config.id.to_string();
    config
        .save_to_default()
        .map_err(|e| format!("Failed to save project: {e}"))?;
    Ok(id)
}

/// List all saved projects.
#[tauri::command]
fn list_projects() -> Result<Vec<ProjectConfig>, String> {
    ProjectConfig::list_all().map_err(|e| format!("Failed to list projects: {e}"))
}

/// Delete a project by ID.
#[tauri::command]
fn delete_project(id: String) -> Result<(), String> {
    ProjectConfig::delete(&id).map_err(|e| format!("Failed to delete project: {e}"))
}

/// Parse SSH config to discover available hosts.
#[tauri::command]
fn parse_ssh_hosts() -> Result<Vec<ssh::SshHost>, String> {
    ssh::parse_ssh_config().map_err(|e| format!("Failed to parse SSH config: {e}"))
}

/// App configuration payload from the onboarding wizard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingRequest {
    pub project_name: String,
    pub ssh_host_alias: String,
    pub remote_root: String,
    pub local_root: String,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub protect_secrets: bool,
}

#[cfg_attr(target_os = "ios", tauri::mobile_entry_point)]
#[cfg_attr(target_os = "android", tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(terminal::TerminalManager::new())
        .manage(ssh_tunnel::TunnelState::new())
        .manage(sync::SyncState::new())
        .invoke_handler(tauri::generate_handler![
            greet,
            save_project,
            list_projects,
            delete_project,
            parse_ssh_hosts,
            terminal::start_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::close_terminal,
            terminal::new_claude_session,
            terminal::close_tmux_window,
            terminal::list_tmux_windows,
            terminal::kill_all_sessions,
            files::browse_files,
            files::read_file,
            files::compute_diff,
            files::open_in_finder,
            ssh_tunnel::create_tunnel,
            ssh_tunnel::tunnel_endpoint,
            ssh_tunnel::close_tunnel,
            sync::start_sync,
            sync::stop_sync,
            sync::sync_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
