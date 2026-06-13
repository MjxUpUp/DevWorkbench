pub mod commands;
pub mod models;
pub mod agents;
pub mod error;
pub mod db;
pub mod migrate;
pub mod activity;
pub mod knowledge;
pub mod config;
pub mod quality;
pub mod mcp;
pub mod skills;
pub mod cost;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            {
                #[cfg(debug_assertions)]
                let log_builder = tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info);
                #[cfg(not(debug_assertions))]
                let log_builder = tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .targets([
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir { file_name: None }),
                    ]);
                app.handle().plugin(log_builder.build())?;
            }

            // Initialize SQLite database
            let data_dir = crate::commands::projects::dirs_home().join(".dev-workbench");
            let db_path = data_dir.join("data.db");
            let conn = db::init_db(&db_path)
                .expect("Failed to initialize database");

            // Run v0.6 → v0.7 migration (idempotent)
            migrate::migrate_v6_to_v7(&conn, &data_dir)
                .expect("Failed to run data migration");

            // Run v0.7 → v0.8 migration (projects/settings to SQLite)
            migrate::migrate_v7_to_v8(&conn, &data_dir)
                .expect("Failed to run projects/settings migration");

            // Run v0.8 → v1.0 migration (workflows, skills, cost tables)
            migrate::migrate_v8_to_v9(&conn)
                .expect("Failed to run v8→v9 schema migration");

            // Prune knowledge entries older than 180 days
            match knowledge::store::prune_old_entries(&conn, 180) {
                Ok(count) => {
                    if count > 0 {
                        log::info!("Pruned {} old knowledge entries", count);
                    }
                }
                Err(e) => {
                    log::warn!("Knowledge prune failed (non-fatal): {}", e);
                }
            }

            // Store the connection as managed state
            let db_state = db::DbState(std::sync::Arc::new(std::sync::Mutex::new(conn)));
            app.manage(db_state.clone());

            // Start knowledge file watchers (background thread)
            match knowledge::watchers::start_knowledge_watchers(db_state.0.clone()) {
                Ok(_guard) => {
                    // WatcherGuard stored in app setup — dropped when app shuts down
                    app.manage(_guard);
                    log::info!("Knowledge watchers started");
                }
                Err(e) => {
                    log::warn!("Knowledge watchers 启动失败 (non-fatal): {}", e);
                }
            }

            Ok(())
        })
        .manage(commands::agents::AgentState(std::sync::Arc::new(
            agents::pty::AgentProcesses::new(),
        )))
        .manage(mcp::registry::McpRegistry::new())
        .invoke_handler(tauri::generate_handler![
            commands::tools::detect_tools,
            commands::terminal::open_terminal,
            commands::terminal::detect_terminals,
            commands::editor::open_in_editor,
            commands::finder::open_in_finder,
            commands::scanner::scan_git_repos,
            commands::scanner::detect_project_tags,
            commands::projects::load_projects,
            commands::projects::add_project,
            commands::projects::remove_project,
            commands::projects::update_project,
            commands::projects::load_settings,
            commands::projects::save_settings,
            commands::projects::update_project_open,
            commands::projects::record_tool_open,
            commands::git::get_git_status,
            commands::git::batch_get_git_status,
            commands::agents::discover_agents_cmd,
            commands::agents::recommend_agent_for_project,
            commands::agents::spawn_agent_session,
            commands::agents::stop_agent_session,
            commands::agents::load_sessions,
            commands::agents::get_sessions_for_project,
            commands::agents::update_session,
            commands::agents::read_session_output_cmd,
            commands::agents::load_requirements,
            commands::agents::add_requirement,
            commands::agents::update_requirement,
            commands::agents::remove_requirement,
            commands::agents::get_requirements_for_project,
            commands::agents::pty_write_cmd,
            commands::agents::pty_resize_cmd,
            commands::agents::get_project_activity,
            commands::agents::get_recent_activity,
            commands::agents::search_knowledge,
            commands::agents::get_knowledge_for_project,
            commands::agents::delete_knowledge_entry,
            commands::agents::trigger_knowledge_collection,
            commands::agents::load_mcp_config,
            commands::agents::save_mcp_config,
            commands::agents::apply_mcp_config,
            commands::agents::get_quality_reports,
            commands::agents::get_quality_report_for_session,
            commands::agents::run_quality_gate,
            commands::files::list_project_files,
            commands::workflows::list_workflows,
            commands::workflows::create_workflow,
            commands::workflows::get_workflow,
            commands::workflows::update_workflow,
            commands::workflows::delete_workflow,
            commands::mcp_cmds::mcp_connect,
            commands::mcp_cmds::mcp_disconnect,
            commands::mcp_cmds::mcp_list_tools,
            commands::mcp_cmds::mcp_call_tool,
            commands::skills_cmds::list_skills,
            commands::skills_cmds::install_skill,
            commands::skills_cmds::uninstall_skill,
            commands::cost_cmds::get_cost_summary,
            commands::cost_cmds::get_cost_trend,
            commands::cost_cmds::load_budget,
            commands::cost_cmds::save_budget,
            commands::cost_cmds::check_budget_alert,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
