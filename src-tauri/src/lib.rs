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
pub mod kernel_impl;
pub mod utils;

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

            // Initialize SQLite database (connection pool + schema + pragmas)
            let data_dir = crate::commands::projects::dirs_home().join(".dev-workbench");
            let db_path = data_dir.join("data.db");
            let db_state = db::DbState::open(&db_path)
                .expect("Failed to initialize database");

            // Run migrations on one pooled connection (idempotent).
            {
                let conn = db_state.get()
                    .expect("Failed to get DB connection from pool for migrations");

                migrate::migrate_v6_to_v7(&conn, &data_dir)
                    .expect("Failed to run data migration");
                migrate::migrate_v7_to_v8(&conn, &data_dir)
                    .expect("Failed to run projects/settings migration");
                migrate::migrate_v8_to_v9(&conn)
                    .expect("Failed to run v8 to v9 schema migration");
                migrate::migrate_v9_to_v10(&conn)
                    .expect("Failed to run v9 to v10 conversation migration");
                migrate::migrate_v10_to_v11(&conn)
                    .expect("Failed to run v10 to v11 blocks column migration");

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
            }

            // Store the pool as managed state
            app.manage(db_state.clone());

            // Start knowledge file watchers (background thread, shares the pool)
            match knowledge::watchers::start_knowledge_watchers(db_state.clone()) {
                Ok(_guard) => {
                    app.manage(_guard);
                    log::info!("Knowledge watchers started");
                }
                Err(e) => {
                    log::warn!("Knowledge watchers failed to start (non-fatal): {}", e);
                }
            }

            Ok(())
        })
        .manage(commands::agents::AgentState(std::sync::Arc::new(
            agents::pty::AgentProcesses::new(),
        )))
        .manage(mcp::registry::McpRegistry::new())
        .manage(commands::workflows::ApprovalState::default())
        .manage(agents::kernel_tasks::KernelTasks::new())
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
            commands::agents::list_conversations,
            commands::agents::update_conversation,
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
            commands::provider_cmds::get_providers_config,
            commands::provider_cmds::set_providers_config,
            commands::provider_cmds::test_provider_connection,
            commands::agents::get_quality_reports,
            commands::agents::get_quality_report_for_session,
            commands::agents::run_quality_gate,
            commands::files::list_project_files,
            commands::workflows::list_workflows,
            commands::workflows::create_workflow,
            commands::workflows::get_workflow,
            commands::workflows::update_workflow,
            commands::workflows::delete_workflow,
            commands::workflows::run_workflow,
            commands::workflows::approve_workflow_step,
            commands::mcp_cmds::mcp_connect,
            commands::mcp_cmds::mcp_disconnect,
            commands::mcp_cmds::mcp_list_tools,
            commands::mcp_cmds::mcp_call_tool,
            commands::mcp_cmds::mcp_catalog,
            commands::mcp_cmds::mcp_servers,
            commands::mcp_cmds::mcp_install_preset,
            commands::skills_cmds::list_skills,
            commands::skills_cmds::install_skill,
            commands::skills_cmds::uninstall_skill,
            commands::skills_cmds::skill_catalog,
            commands::skills_cmds::install_skill_from_catalog,
            commands::skills_cmds::rate_skill,
            commands::cost_cmds::get_cost_summary,
            commands::cost_cmds::get_cost_trend,
            commands::cost_cmds::load_budget,
            commands::cost_cmds::save_budget,
            commands::cost_cmds::check_budget_alert,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
