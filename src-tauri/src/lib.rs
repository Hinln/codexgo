mod api;
mod application;
mod codex;
mod commands;
mod errors;
mod provider;
mod security;
mod storage;
mod windows;

use commands::ApplicationState;

#[cfg(test)]
mod tests;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ApplicationState::default())
        .invoke_handler(tauri::generate_handler![
            commands::detect_status,
            commands::fetch_models,
            commands::switch_provider,
            commands::restore_official,
            commands::delete_saved_key,
            commands::open_codex_home,
            commands::open_backup_directory,
            commands::open_vexlune_hub,
            commands::clear_logs
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Codex API Switcher");
}
