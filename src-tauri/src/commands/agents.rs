use crate::agents::discovery::{discover_agents, recommend_agent, AgentInfo};
use crate::agents::pty;
use crate::agents::session;
use crate::db::DbState;
use crate::error::AppError;
use crate::models::{AgentType, Conversation, Session};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use crate::agents::kernel_tasks::KernelTasks;
use crate::agents::react_chat;
use crate::kernel_impl::executor;
use crate::mcp::registry::McpRegistry;
use crate::models::SessionStatus;
use kernel_core::{Agent, AgentEvent, AgentInput, AgentRunStatus};

/// Tauri managed state wrapping AgentProcesses (PTY-based)
pub struct AgentState(pub Arc<pty::AgentProcesses>);

// Agent discovery commands
#[tauri::command]
pub fn discover_agents_cmd(db: State<'_, DbState>) -> Result<Vec<AgentInfo>, AppError> {
    let conn = db.get()?;
    Ok(discover_agents(Some(&conn)))
}

#[tauri::command]
pub fn recommend_agent_for_project(tags: Vec<String>) -> Result<Option<AgentType>, AppError> {
    Ok(recommend_agent(&tags))
}

// Session commands
#[tauri::command]
pub fn load_sessions(db: State<'_, DbState>) -> Result<Vec<Session>, AppError> {
    let conn = db.get()?;
    crate::agents::session::load_sessions_from_db(&conn).map_err(AppError::from)
}

/// Read the FULL (ANSI-stripped) output for a session, for the completed-session terminal view.
/// Unlike the stored `outputSummary` (tail-truncated to OUTPUT_SUMMARY_MAX_CHARS), this returns
/// the complete text so the completed session isn't cut off mid-reply.
#[tauri::command]
pub fn read_session_output_cmd(session_id: String) -> Result<Option<String>, AppError> {
    Ok(pty::read_full_session_output(&session_id))
}

// Agent process lifecycle commands (PTY-based for CLI agents + kernel for the
// self-hosted ReactAgent).
#[tauri::command]
pub async fn spawn_agent_session(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
    db: State<'_, DbState>,
    kernel_tasks: State<'_, KernelTasks>,
    project_path: String,
    agent_type: AgentType,
    prompt: String,
    model: Option<String>,
    linked_requirement_id: Option<String>,
    parent_session_id: Option<String>,
    conversation_id: Option<String>,
    kernel: bool,
    mode: Option<crate::kernel_impl::hooks::PermissionMode>,
) -> Result<Session, AppError> {
    // MUST be `async`: react_chat_driver calls `tokio::spawn`, which requires a
    // current Tokio runtime context. A sync Tauri command runs on the main
    // thread (NO runtime context) → `tokio::spawn` panics with "there is no
    // reactor running", and because that panic is on the main thread it can't
    // unwind across the Tauri/webview FFI boundary → process aborts (闪退). The
    // async command body is driven on Tauri's tokio runtime, so the runtime
    // context is present and `tokio::spawn` succeeds. The CLI path
    // (spawn_pty_agent) uses `std::thread::spawn` and never had this issue.
    if kernel {
        // Self-hosted ReactAgent path: no child process, no PTY. The agent runs
        // as a tokio task driving a BoxStream<AgentEvent>, mapped to the SAME
        // `agent:event` wire schema claude uses (see react_chat). This is the B
        // plan's core payoff — one chat-block presentation layer for both CLI
        // and self-hosted agents.
        return Ok(react_chat_driver(
            &app,
            db.inner().clone(),
            kernel_tasks.inner(),
            &project_path,
            &agent_type,
            &prompt,
            model.as_deref(),
            linked_requirement_id.as_deref(),
            parent_session_id.as_deref(),
            conversation_id.as_deref(),
            mode.unwrap_or_default(),
        )?);
    }
    Ok(pty::spawn_pty_agent(
        &app,
        state.0.clone(),
        db.inner().clone(),
        &project_path,
        agent_type,
        &prompt,
        model.as_deref(),
        linked_requirement_id.as_deref(),
        parent_session_id.as_deref(),
        conversation_id.as_deref(),
    )?)
}

/// Drive a self-hosted ReactAgent on a background tokio task, mirroring the pty
/// spawn shape: build the Running session row synchronously (so the UI gets a
/// session id + `agent:started` immediately), then stream AgentEvents →
/// ChatStreamEvents → `agent:event`, and finalize on completion/error.
///
/// The driver task is registered in `KernelTasks` so `stop_agent_session` can
/// abort it. Aborting drops the future — the driver's own finalize does NOT run
/// — so stop must write the failed status itself, which it does.
///
/// MVP constraints (explicitly deferred, see plan §"阶段 E 后续"):
/// empty ToolRegistry (no real tools yet), non-streaming `generate()` (Token is
/// the whole message, not chunked), single-turn (resume_from ignored).
fn react_chat_driver(
    app: &tauri::AppHandle,
    db_conn: crate::db::DbState,
    kernel_tasks: &KernelTasks,
    project_path: &str,
    agent_type: &AgentType,
    prompt: &str,
    model: Option<&str>,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    conversation_id: Option<&str>,
    mode: crate::kernel_impl::hooks::PermissionMode,
) -> Result<Session, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    log::info!(
        "[react_chat] driver START sid={session_id} agent={agent_type:?} model={model:?} conv={conversation_id:?}"
    );
    let resolved_conv_id = pty::resolve_or_create_conversation(
        &db_conn, conversation_id, project_path, prompt, agent_type,
    )?;
    let session = pty::build_running_session_row(
        &session_id, project_path, agent_type, prompt, model,
        &resolved_conv_id, linked_requirement_id, parent_session_id,
    );
    pty::register_running_session(
        &db_conn, app, &session, conversation_id, &resolved_conv_id, project_path, agent_type,
    )?;

    // Shadow-git checkpoint at session start: snapshot the working tree so the
    // user can roll this agent's changes back later. Best-effort — a non-repo
    // path or missing git only disables rollback for this session, it never
    // blocks the run (checkpoint is an enhancement, not a gate).
    if let Err(e) = crate::kernel_impl::checkpoint::create_at_session_start(
        project_path,
        &session_id,
        "session_start",
    ) {
        log::warn!("[checkpoint] create failed for {session_id}: {e} (rollback disabled)");
    }

    // T8 experience flywheel replay: pull Forge's pending *mandatory* reviews
    // (low-score, unresolved) into the knowledge base as quality_failure lessons,
    // which build_react_agent → experience_prompt_suffix (T7) surfaces in THIS
    // session's system prompt. Best-effort + dedup → idempotent and non-blocking
    // (forge missing / no pending reviews = no-op). Same blocking-subprocess-at-
    // session-start pattern as the checkpoint above.
    match crate::quality::experience::list_forge_reviews(std::path::Path::new(project_path)) {
        Ok(reviews) => {
            let pending = crate::quality::experience::pending_mandatory(&reviews);
            if !pending.is_empty() {
                if let Ok(conn) = db_conn.get() {
                    let res = crate::quality::experience::replay_to_knowledge(
                        &conn, project_path, &pending, agent_type,
                    );
                    log::info!(
                        "[experience] replayed {} lessons ({} skipped) for {session_id}",
                        res.replayed, res.skipped
                    );
                }
            }
        }
        Err(crate::error::AppError::ForgeNotInstalled) => {} // forge absent → nothing new
        Err(e) => log::warn!("[experience] replay failed for {session_id}: {e}"),
    }

    // Prior conversation turns → structured Message history. This is the
    // ReactAgent analog of the CLI path's inject_conversation_context, but built
    // from persisted blocks (real user/assistant/tool turns) instead of a flat
    // output_summary string. Computed before spawn so it can be moved into the
    // task. load_prior_turns returns ASC; the current turn (just registered as
    // Running) is excluded by turns_to_history's Running filter.
    let prior_turns = pty::load_prior_turns(&db_conn, &resolved_conv_id);
    let history_drv = react_chat::turns_to_history(
        &prior_turns,
        react_chat::REACT_HISTORY_TURN_TEXT_CAP,
        react_chat::REACT_HISTORY_TOTAL_TEXT_CAP,
    );

    let started = std::time::Instant::now();
    let app_drv = app.clone();
    let sid_drv = session_id.clone();
    let db_drv = db_conn.clone();
    let pp_drv = project_path.to_string();
    let at_drv = agent_type.clone();
    let model_drv = model.map(|m| m.to_string());
    let prompt_drv = prompt.to_string();
    let conv_drv = resolved_conv_id.clone();

    let handle = tokio::spawn(async move {
        log::info!("[react_chat] task ENTERED sid={sid_drv}");
        let mcp = app_drv.try_state::<McpRegistry>();
        let agent = match executor::build_react_agent(
            model_drv.as_deref(),
            mcp.as_deref(),
            &pp_drv,
            Some(conv_drv.as_str()),
            history_drv,
            Some(db_drv.clone()),
            mode,
        ) {
            Ok(a) => a,
            Err(e) => {
                log::error!("[react_chat] build_react_agent failed for {}: {e}", sid_drv);
                pty::finalize_session(
                    &db_drv, &app_drv, &sid_drv, &pp_drv, &at_drv,
                    SessionStatus::Failed, None,
                    Some(format!("Agent init failed: {e}")), None, None,
                );
                return;
            }
        };
        log::info!("[react_chat] build_react_agent OK sid={sid_drv}");
        let input = AgentInput {
            prompt: prompt_drv.clone(),
            working_dir: Some(pp_drv.clone()),
            model: None,
            resume_from: None,
        };
        let mut stream = match agent.run(input) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[react_chat] agent.run failed for {}: {e}", sid_drv);
                pty::finalize_session(
                    &db_drv, &app_drv, &sid_drv, &pp_drv, &at_drv,
                    SessionStatus::Failed, None,
                    Some(format!("Agent run failed: {e}")), None, None,
                );
                return;
            }
        };
        log::info!("[react_chat] agent.run OK, streaming sid={sid_drv}");

        use futures::StreamExt;
        let mut final_status = SessionStatus::Completed;
        let mut final_exit: Option<i32> = Some(0);
        let mut final_output = String::new();
        // Accumulate the wire events emitted to agent:event so the completed
        // session can be persisted and replayed via BlocksView. Mirrors the
        // pipe path's Arc<Mutex<Vec>> accumulation.
        let mut final_blocks: Vec<pty::ChatStreamEvent> = Vec::new();
        while let Some(ev_res) = stream.next().await {
            let secs = started.elapsed().as_secs();
            let ev = match ev_res {
                Ok(e) => e,
                Err(e) => {
                    log::error!("[react_chat] stream error for {}: {e}", sid_drv);
                    let summary = if final_output.is_empty() {
                        Some(format!("Agent error: {e}"))
                    } else {
                        Some(final_output.clone())
                    };
                    pty::finalize_session(
                        &db_drv, &app_drv, &sid_drv, &pp_drv, &at_drv,
                        SessionStatus::Failed, None, summary, None, None,
                    );
                    return;
                }
            };
            // Track terminal status/exit/output from Done; accumulate Token text
            // as a fallback summary (Done.output_summary is authoritative when set).
            match &ev {
                AgentEvent::Token(t) => final_output.push_str(t),
                AgentEvent::Done(outcome) => {
                    final_status = match outcome.status {
                        AgentRunStatus::Completed => SessionStatus::Completed,
                        _ => SessionStatus::Failed,
                    };
                    final_exit = outcome.exit_code;
                    if let Some(s) = &outcome.output_summary {
                        final_output = s.clone();
                    }
                }
                _ => {}
            }
            let wires = react_chat::map_agent_event(ev, secs);
            for wire in &wires {
                let _ = app_drv.emit(
                    "agent:event",
                    serde_json::json!({ "sessionId": &sid_drv, "event": wire }),
                );
            }
            final_blocks.extend(wires);
        }

        // Stream ended (ReactAgent always yields Done before ending — this is the
        // normal completion path). Remove from the stop table so a later stop on
        // this completed session doesn't touch a dead handle, then persist state.
        log::info!(
            "[react_chat] stream ENDED sid={sid_drv} status={final_status:?} blocks={}",
            final_blocks.len()
        );
        if let Some(kt) = app_drv.try_state::<KernelTasks>() {
            kt.remove(&sid_drv);
        }
        let summary = if final_output.is_empty() { None } else { Some(final_output) };

        // v1.3 T2: close the long-term-memory loop. The opaque CLI path feeds
        // the knowledge flywheel via collect_from_session (log-parsing); the
        // kernel agent has no log, so write its completed output directly as a
        // `react_session` entry → the NEXT session's memory_prompt_suffix can
        // surface it. Best-effort: a DB write failure just logs + continues
        // (memory is an enhancement, not a gate). Only on Completed so a failed/
        // degraded run doesn't pollute the project's memory.
        if final_status == SessionStatus::Completed {
            if let Some(out) = summary.as_ref() {
                if let Ok(conn) = db_drv.get() {
                    let hash = crate::activity::hash_project_path(&pp_drv);
                    let entry = crate::knowledge::store::build_session_memory_entry(
                        &hash, &sid_drv, &prompt_drv, out, &at_drv,
                    );
                    if let Err(e) = crate::knowledge::store::add_entry(&conn, &entry) {
                        log::warn!("[react_chat] session-memory write failed for {sid_drv}: {e}");
                    } else {
                        log::info!("[react_chat] session-memory recorded for {sid_drv}");
                    }
                }
            }
        }

        pty::finalize_session(
            &db_drv, &app_drv, &sid_drv, &pp_drv, &at_drv,
            final_status, final_exit, summary, None,
            Some(final_blocks),
        );
    });

    kernel_tasks.insert(&session_id, handle);
    Ok(session)
}

// ---- Conversation commands ----
//
// A conversation is the multi-turn container. spawn_agent_session attaches a
// turn (creating the conversation when conversation_id is None); these commands
// cover listing / renaming / archiving for the sidebar.

#[tauri::command]
pub fn list_conversations(db: State<'_, DbState>, project_path: String) -> Result<Vec<Conversation>, AppError> {
    let conn = db.get()?;
    session::load_conversations_for_project_db(&conn, &project_path).map_err(AppError::from)
}

#[tauri::command]
pub fn update_conversation(
    db: State<'_, DbState>,
    id: String,
    patch: serde_json::Value,
) -> Result<(), AppError> {
    let conn = db.get()?;
    session::update_conversation_db(&conn, &id, patch).map_err(AppError::from)
}

#[tauri::command]
pub fn stop_agent_session(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
    db: State<'_, DbState>,
    kernel_tasks: State<'_, KernelTasks>,
    session_id: String,
) -> Result<(), AppError> {
    // Kernel agents have no PID — abort their driver task. Returns true iff this
    // session was a kernel task, in which case we skip the pty/PID kill below.
    let was_kernel = kernel_tasks.abort(&session_id);
    if !was_kernel {
        // Best-effort PID kill; process may already be dead (stale session)
        let _ = pty::stop_agent(&state.0, &session_id);
    }
    // Aborting a kernel task drops its future — the driver's own finalize does
    // NOT run. So we always write the failed status + emit agent:completed
    // here (same as the pty path), regardless of agent kind.

    // Always update session status so UI reflects the stop immediately
    let patch = serde_json::json!({
        "status": "failed",
        "finishedAt": chrono::Utc::now().to_rfc3339(),
        "exitCode": -1,
        "outputSummary": "Session stopped by user"
    });
    {
        let conn = db.get()?;
        crate::agents::session::update_session_db(&conn, &session_id, patch)?;
    }

    let _ = app.emit(
        "agent:completed",
        serde_json::json!({
            "sessionId": session_id,
            "status": "failed",
            "exitCode": -1
        }),
    );

    Ok(())
}

#[tauri::command]
pub fn pty_write_cmd(
    state: State<'_, AgentState>,
    session_id: String,
    data: String,
) -> Result<(), AppError> {
    pty::pty_write(&state.0, &session_id, &data).map_err(AppError::from)
}

#[tauri::command]
pub fn pty_resize_cmd(
    state: State<'_, AgentState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), AppError> {
    pty::pty_resize(&state.0, &session_id, cols, rows).map_err(AppError::from)
}

// Activity commands
#[tauri::command]
pub fn get_project_activity(db: State<'_, DbState>, project_path: String) -> Result<Vec<crate::models::ActivityEvent>, AppError> {
    let conn = db.get()?;
    crate::activity::get_events_for_project(&conn, &project_path).map_err(AppError::from)
}

#[tauri::command]
pub fn get_recent_activity(db: State<'_, DbState>, limit: Option<usize>) -> Result<Vec<crate::models::ActivityEvent>, AppError> {
    let conn = db.get()?;
    crate::activity::get_recent_events(&conn, limit.unwrap_or(50)).map_err(AppError::from)
}

// Knowledge commands
#[tauri::command]
pub fn search_knowledge(db: State<'_, DbState>, query: String, limit: Option<usize>) -> Result<Vec<crate::models::KnowledgeEntry>, AppError> {
    let conn = db.get()?;
    crate::knowledge::store::search_entries(&conn, &query, limit.unwrap_or(20)).map_err(AppError::from)
}

#[tauri::command]
pub fn get_knowledge_for_project(db: State<'_, DbState>, project_path: String) -> Result<Vec<crate::models::KnowledgeEntry>, AppError> {
    let conn = db.get()?;
    let hash = crate::activity::hash_project_path(&project_path);
    crate::knowledge::store::get_entries_for_project(&conn, &hash).map_err(AppError::from)
}

#[tauri::command]
pub fn delete_knowledge_entry(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.get()?;
    crate::knowledge::store::delete_entry(&conn, &id).map_err(AppError::from)
}

// Config commands
#[tauri::command]
pub fn load_mcp_config(project_path: String) -> Result<crate::models::McpConfigFile, AppError> {
    let path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    if !path.exists() {
        return Ok(crate::models::McpConfigFile { servers: vec![] });
    }
    crate::config::mcp::load_mcp_config(&path).map_err(AppError::from)
}

#[tauri::command]
pub fn save_mcp_config(project_path: String, config: crate::models::McpConfigFile) -> Result<(), AppError> {
    let path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    crate::config::mcp::save_mcp_config(&config, &path).map_err(AppError::from)
}

#[tauri::command]
pub fn apply_mcp_config(project_path: String, config: crate::models::McpConfigFile) -> Result<Vec<String>, AppError> {
    let path = std::path::Path::new(&project_path);
    crate::config::adapters::apply_translations(&config, path).map_err(AppError::from)
}

// Quality commands
#[tauri::command]
pub fn get_quality_reports(db: State<'_, DbState>) -> Result<Vec<crate::models::QualityReport>, AppError> {
    let conn = db.get()?;
    crate::quality::report::get_all_reports(&conn).map_err(AppError::from)
}

#[tauri::command]
pub fn get_quality_report_for_session(db: State<'_, DbState>, session_id: String) -> Result<Option<crate::models::QualityReport>, AppError> {
    let conn = db.get()?;
    crate::quality::report::get_report_for_session(&conn, &session_id).map_err(AppError::from)
}

