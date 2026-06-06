mod commands;
mod models;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::tools::detect_tools,
            commands::terminal::open_terminal,
            commands::editor::open_in_editor,
            commands::finder::open_in_finder,
            commands::scanner::scan_git_repos,
            commands::projects::load_projects,
            commands::projects::save_projects,
            commands::projects::load_settings,
            commands::projects::save_settings,
            commands::projects::update_project_open,
            commands::git::get_git_status,
            commands::git::batch_get_git_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
