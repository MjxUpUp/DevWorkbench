pub mod acp;
pub mod activity;
pub mod agents;
pub mod commands;
pub mod config;
pub mod cost;
pub mod db;
pub mod error;
pub mod eval;
pub mod kernel_impl;
pub mod knowledge;
pub mod mcp;
pub mod migrate;
pub mod models;
pub mod quality;
pub mod skills;
pub mod slash_commands;
pub mod trace;
pub mod user_hooks;
pub mod utils;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Global panic hook — captures EVERY panic (main thread + tokio worker)
    // with its file:line:col location, payload message, and a forced backtrace,
    // and writes it to the app log. This is the only way to localize a panic
    // that doesn't surface as an error string (e.g. an unwinding tokio task that
    // leaves a session stuck in Running, or a hard process exit on the main
    // thread). Installed before the builder so it covers the whole app lifetime;
    // log::error! resolves at panic time, by which point tauri_plugin_log is up.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let bt = std::backtrace::Backtrace::force_capture();
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line =
            format!("[{secs}] [PANIC] thread panicked at {location}: {msg}\nbacktrace:\n{bt}");
        log::error!("{line}");
        eprintln!("{line}");
        // Flush-safe mirror of the panic to a dedicated file — the buffered
        // tauri_plugin_log target can lose its last line on a hard crash (panic
        // on a thread that can't unwind, e.g. across the Tauri/webview FFI
        // boundary, which aborts the process). The hook fires BEFORE the abort,
        // so this flush() captures the panic even then. This is what made the
        // Kernel-Agent 闪退 ("there is no reactor running" at a sync-command
        // tokio::spawn) diagnosable in the first place.
        let diag = crate::commands::projects::dirs_home()
            .join(".dev-workbench")
            .join("panic.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&diag)
        {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }));

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
                let log_builder =
                    tauri_plugin_log::Builder::default().level(log::LevelFilter::Info);
                #[cfg(not(debug_assertions))]
                let log_builder = tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .targets([tauri_plugin_log::Target::new(
                        tauri_plugin_log::TargetKind::LogDir { file_name: None },
                    )]);
                app.handle().plugin(log_builder.build())?;
            }

            // Initialize SQLite database (connection pool + schema + pragmas).
            // F15: any failure here previously panic'd via .expect, so the app
            // vanished on startup with no UI — only a panic.log the user would
            // never think to read. Now we surface a blocking native dialog with
            // the stage + error + actionable suggestions, THEN exit(1). Still a
            // hard failure (we do NOT pretend to succeed and run on a broken
            // DB), just no longer a silent crash. db_state.open and every
            // migration are covered by this guard.
            let data_dir = crate::commands::projects::dirs_home().join(".dev-workbench");
            let db_path = data_dir.join("data.db");
            use tauri_plugin_dialog::DialogExt;
            let fail = |app: &tauri::App, stage: &str, e: crate::error::AppError| -> ! {
                let msg = format_migration_error(stage, &e);
                log::error!("Startup abort at stage `{stage}`: {e}");
                app.dialog()
                    .message(msg)
                    .title("数据库初始化失败")
                    .blocking_show();
                std::process::exit(1);
            };
            let db_state = match db::DbState::open(&db_path) {
                Ok(s) => s,
                Err(e) => fail(app, "open DB pool", e),
            };

            // Run migrations on one pooled connection (idempotent).
            {
                let conn = match db_state.get() {
                    Ok(c) => c,
                    Err(e) => fail(app, "acquire migration connection", e.into()),
                };

                // Helper macro so each migration maps its AppError to a dialog+exit
                // instead of .expect. Inline closure would also work; the macro
                // preserves the per-stage label with less ceremony.
                macro_rules! run_migrate {
                    ($stage:expr, $call:expr) => {
                        match $call {
                            Ok(()) => {}
                            Err(e) => fail(app, $stage, e),
                        }
                    };
                }
                run_migrate!(
                    "v6→v7 data migration",
                    migrate::migrate_v6_to_v7(&conn, &data_dir)
                );
                run_migrate!(
                    "v7→v8 projects/settings migration",
                    migrate::migrate_v7_to_v8(&conn, &data_dir)
                );
                run_migrate!("v8→v9 schema migration", migrate::migrate_v8_to_v9(&conn));
                run_migrate!(
                    "v9→v10 conversation migration",
                    migrate::migrate_v9_to_v10(&conn)
                );
                run_migrate!(
                    "v10→v11 blocks column migration",
                    migrate::migrate_v10_to_v11(&conn)
                );
                run_migrate!(
                    "v11→v12 task_ref column migration",
                    migrate::migrate_v11_to_v12(&conn)
                );
                run_migrate!(
                    "v12→v13 user_hooks.matcher column migration",
                    migrate::migrate_v12_to_v13(&conn)
                );
                run_migrate!(
                    "v13→v14 llm_traces table migration",
                    migrate::migrate_v13_to_v14(&conn)
                );
                run_migrate!(
                    "v14→v15 trace_settings + index migration",
                    migrate::migrate_v14_to_v15(&conn)
                );
                run_migrate!(
                    "v15→v16 eval_runs migration",
                    migrate::migrate_v15_to_v16(&conn)
                );
                run_migrate!(
                    "v16→v17 cost cache-columns migration",
                    migrate::migrate_v16_to_v17(&conn)
                );
                run_migrate!(
                    "v17→v18 trace timing-columns migration",
                    migrate::migrate_v17_to_v18(&conn)
                );
                run_migrate!(
                    "v18→v19 settings.palette column migration",
                    migrate::migrate_v18_to_v19(&conn)
                );
                run_migrate!(
                    "v19→v20 settings.onboarding_completed column migration",
                    migrate::migrate_v19_to_v20(&conn)
                );
                run_migrate!(
                    "v20→v21 verdicts ledger migration",
                    migrate::migrate_v20_to_v21(&conn)
                );
                run_migrate!(
                    "v21→v22 eval_cases migration",
                    migrate::migrate_v21_to_v22(&conn)
                );

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
                // Lazy trace retention (2026-06-19 observability research): local
                // apps aren't long-running, so TTL runs on startup, not a cron.
                // Prune traces past their retention window, then VACUUM (throttled
                // weekly) to reclaim disk — SQLite doesn't reclaim after DELETE.
                // Both best-effort: a failure here must never block app startup.
                match trace::db::get_trace_settings(&conn) {
                    Ok(settings) => {
                        match trace::db::prune_old_traces(&conn, settings.retention_days) {
                            Ok(n) if n > 0 => log::info!("Pruned {} old llm_traces", n),
                            Ok(_) => {}
                            Err(e) => log::warn!("llm_traces prune failed (non-fatal): {e}"),
                        }
                        if let Err(e) = trace::db::maybe_vacuum(&conn, &settings) {
                            log::warn!("llm_traces vacuum failed (non-fatal): {e}");
                        }
                    }
                    Err(e) => log::warn!("trace_settings read failed (non-fatal): {e}"),
                }
            }

            // Store the pool as managed state
            app.manage(db_state.clone());

            // L1 verdict ledger — drain circuit-breaker transitions (TRIPPED /
            // RESET) into the verdicts table. The breaker emits to a global
            // channel (cost/ stays db-agnostic); this consumer is the only thing
            // that touches the db for circuit events. Best-effort: a failed
            // insert is logged, never blocks the breaker or the request path.
            {
                let (tx, mut rx) =
                    tokio::sync::mpsc::unbounded_channel::<cost::circuit_breaker::CircuitEvent>();
                cost::circuit_breaker::set_event_sender(tx);
                let db_for_circuit = db_state.clone();
                tauri::async_runtime::spawn(async move {
                    while let Some(evt) = rx.recv().await {
                        let verdict = match evt.kind {
                            cost::circuit_breaker::CircuitEventKind::Tripped => "TRIPPED",
                            cost::circuit_breaker::CircuitEventKind::Reset => "RESET",
                        };
                        let row = eval::verdicts::NewVerdict {
                            id: uuid::Uuid::new_v4().to_string(),
                            session_id: None,
                            case_id: None,
                            gate: "circuit-breaker".to_string(),
                            verdict: verdict.to_string(),
                            // A host outage/recovery is not "your work" — no
                            // anti-gaming attribution on circuit events.
                            attribution: None,
                            report: Some(
                                serde_json::to_string(&serde_json::json!({"host": evt.host}))
                                    .unwrap_or_else(|_| "{}".into()),
                            ),
                            commit_sha: None,
                            created_at: chrono::Utc::now().to_rfc3339(),
                        };
                        let db = db_for_circuit.clone();
                        let _ = tauri::async_runtime::spawn_blocking(move || {
                            if let Ok(conn) = db.get() {
                                if let Err(e) = eval::verdicts::insert_verdict(&conn, &row) {
                                    log::warn!("[verdict-ledger] circuit event persist failed: {e}");
                                }
                            }
                        })
                        .await;
                    }
                });
                log::info!("Circuit-breaker verdict consumer started");
            }

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
        .manage(commands::agents::AgentApprovalState::default())
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
            commands::git::list_changed_files,
            commands::git::get_file_diff,
            commands::checkpoint::get_checkpoint,
            commands::checkpoint::rollback_to_checkpoint,
            commands::agents::discover_agents_cmd,
            commands::agents::recommend_agent_for_project,
            commands::agents::spawn_agent_session,
            commands::mission::mission_init,
            commands::mission::mission_load_prd,
            commands::mission::mission_apply,
            commands::mission::mission_status,
            commands::eval::eval_run_session,
            commands::eval::list_eval_runs,
            commands::eval::eval_trend,
            commands::eval::list_verdicts,
            commands::eval::list_eval_cases,
            commands::eval::get_eval_case,
            commands::eval::approve_eval_case,
            commands::eval::create_eval_case,
            commands::eval::update_eval_case,
            commands::eval::run_eval_replay,
            commands::eval::preview_session_trajectory,
            commands::eval::score_eval_rubric,
            commands::eval::eval_platform_mechanism,
            commands::eval::eval_platform_e2e,
            commands::eval::eval_platform_coverage,
            commands::eval::run_eval_enablement,
            commands::agents::stop_agent_session,
            commands::agents::load_sessions,
            commands::agents::read_session_output_cmd,
            commands::agents::read_compact_archive_cmd,
            commands::agents::resolve_human_gate_cmd,
            commands::agents::list_conversations,
            commands::agents::update_conversation,
            commands::agents::archive_conversation,
            commands::agents::delete_conversation,
            commands::agents::restore_conversation,
            commands::agents::edit_and_regenerate,
            commands::agents::get_conversation_branches,
            commands::agents::pty_write_cmd,
            commands::agents::pty_resize_cmd,
            commands::agents::get_project_activity,
            commands::agents::get_recent_activity,
            commands::agents::search_knowledge,
            commands::agents::get_knowledge_for_project,
            commands::agents::delete_knowledge_entry,
            commands::agents::update_knowledge_entry,
            commands::agents::load_mcp_config,
            commands::agents::save_mcp_config,
            commands::agents::apply_mcp_config,
            commands::provider_cmds::get_providers_config,
            commands::provider_cmds::set_providers_config,
            commands::provider_cmds::test_provider_connection,
            commands::agents::get_quality_reports,
            commands::agents::get_quality_report_for_session,
            commands::experience::list_pending_forge_reviews,
            commands::experience::replay_forge_experience,
            commands::workflows::list_workflows,
            commands::workflows::create_workflow,
            commands::workflows::update_workflow,
            commands::workflows::delete_workflow,
            commands::workflows::run_workflow,
            commands::workflows::approve_workflow_step,
            commands::workflows::list_workflow_templates,
            commands::mcp_cmds::mcp_connect,
            commands::mcp_cmds::mcp_disconnect,
            commands::mcp_cmds::mcp_call_tool,
            commands::mcp_cmds::mcp_catalog,
            commands::mcp_cmds::mcp_servers,
            commands::mcp_cmds::mcp_set_enabled,
            commands::mcp_cmds::mcp_update_server,
            commands::mcp_cmds::mcp_delete_server,
            commands::mcp_cmds::mcp_load_enabled,
            commands::skills_cmds::list_skills,
            commands::skills_cmds::uninstall_skill,
            commands::skills_cmds::skill_catalog,
            commands::skills_cmds::install_skill_from_catalog,
            commands::slash_cmds::list_slash_commands,
            commands::slash_cmds::render_slash_command,
            commands::slash_cmds::create_slash_command,
            commands::slash_cmds::update_slash_command,
            commands::slash_cmds::delete_slash_command,
            commands::user_hooks_cmds::list_user_hooks,
            commands::user_hooks_cmds::create_user_hook,
            commands::user_hooks_cmds::update_user_hook,
            commands::user_hooks_cmds::set_user_hook_enabled,
            commands::user_hooks_cmds::delete_user_hook,
            commands::subagents_cmds::list_subagents,
            commands::subagents_cmds::save_subagent,
            commands::subagents_cmds::delete_subagent,
            commands::cost_cmds::get_cost_summary,
            commands::cost_cmds::get_cost_trend,
            commands::cost_cmds::load_budget,
            commands::cost_cmds::save_budget,
            commands::trace::list_llm_traces,
            commands::trace::get_trace_settings_cmd,
            commands::trace::set_trace_retention_cmd,
            commands::trace::prune_llm_traces_now,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // B3: on exit, kill every tracked agent child so closing the window
            // while an agent is running doesn't orphan the CLI processes
            // (claude/codex/gemini keep holding file locks + burning API quota).
            // `taskkill /F /T` takes the whole tree, MCP grandchildren included.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(state) = app.try_state::<commands::agents::AgentState>() {
                    state.inner().0.kill_all();
                }
            }
        });
}

/// Format a migration / DB-init failure into a user-facing message for the
/// blocking native dialog shown before exit. Extracted as a pure function so
/// the wording (error + actionable suggestion) is unit-testable without
/// spinning up a Tauri AppHandle (which would be required to mock
/// `app.dialog()`). The dialog title is set by the caller; this returns ONLY
/// the body.
///
/// Kept deliberately low-tech (no i18n, no templating) — the dialog is the last
/// line of defense before a hard exit, so it must not itself be able to fail.
fn format_migration_error(stage: &str, e: &crate::error::AppError) -> String {
    format!(
        "数据库初始化在「{stage}」阶段失败，应用无法继续启动。\n\n\
         错误详情：{e}\n\n\
         你可以尝试：\n\
         1. 备份 ~/.dev-workbench/data.db 后删除它（会重置本地数据，但不影响代码）\n\
         2. 检查磁盘空间和文件权限\n\
         3. 联系支持并附上 ~/.dev-workbench/panic.log",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;

    /// F15: the message names the failing stage so the user (and support reading
    /// a screenshot) can localize whether it was the pool open, a specific
    /// migration, etc. — instead of a generic "something broke".
    #[test]
    fn format_migration_error_includes_stage_and_error_message() {
        let e = AppError::Db(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(14), // SQLITE_CANTOPEN
            Some("disk I/O error".into()),
        ));
        let msg = format_migration_error("v9→v10 conversation migration", &e);
        assert!(
            msg.contains("v9→v10 conversation migration"),
            "stage must be in the message: {msg}"
        );
        assert!(
            msg.contains("Database error") || msg.contains("disk I/O error"),
            "underlying error text must be in the message: {msg}"
        );
    }

    /// F15: the message must carry the actionable suggestions (backup path,
    /// disk, panic.log) so the dialog is actually useful, not just a panic line.
    /// Pinning the three suggestion lines guards against a future "trim the
    /// dialog text" change that drops them.
    #[test]
    fn format_migration_error_includes_actionable_suggestions() {
        let e = AppError::Internal("schema conflict".into());
        let msg = format_migration_error("schema migration", &e);
        assert!(msg.contains("data.db"), "must point at the DB file: {msg}");
        assert!(
            msg.contains("panic.log"),
            "must mention the log file: {msg}"
        );
        assert!(
            msg.contains("磁盘空间") || msg.contains("disk"),
            "must suggest disk check: {msg}"
        );
    }

    /// F15: every AppError variant carries a Display impl (thiserror) — the
    /// formatter must not panic on any of them. Sanity check with a couple of
    /// the variants that actually flow through migrations (Db, Io, Internal).
    #[test]
    fn format_migration_error_handles_multiple_variants_without_panic() {
        let cases = [
            AppError::Db(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                None,
            )),
            AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, "lock")),
            AppError::Internal("pool exhausted".into()),
        ];
        for e in cases {
            let msg = format_migration_error("any stage", &e);
            assert!(!msg.is_empty());
            assert!(msg.contains("any stage"));
        }
    }
}
