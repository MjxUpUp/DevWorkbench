use crate::agents::discovery::{discover_agents, recommend_agent, AgentInfo};
use crate::agents::pty;
use crate::db::DbState;
use crate::models::{AgentType, Requirement, Session};
use std::sync::Arc;
use tauri::{Emitter, State};

/// Tauri managed state wrapping AgentProcesses (PTY-based)
pub struct AgentState(pub Arc<pty::AgentProcesses>);

// Agent discovery commands
#[tauri::command]
pub fn discover_agents_cmd(db: State<'_, DbState>) -> Result<Vec<AgentInfo>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    Ok(discover_agents(Some(&conn)))
}

#[tauri::command]
pub fn recommend_agent_for_project(tags: Vec<String>) -> Result<Option<AgentType>, String> {
    Ok(recommend_agent(&tags))
}

// Session commands
#[tauri::command]
pub fn load_sessions(db: State<'_, DbState>) -> Result<Vec<Session>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::session::load_sessions_from_db(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_sessions_for_project(db: State<'_, DbState>, project_path: String) -> Result<Vec<Session>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::session::get_sessions_for_project_db(&conn, &project_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_session(db: State<'_, DbState>, id: String, patch: serde_json::Value) -> Result<(), String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::session::update_session_db(&conn, &id, patch).map_err(|e| e.to_string())
}

/// Read the FULL (ANSI-stripped) output for a session, for the completed-session terminal view.
/// Unlike the stored `outputSummary` (tail-truncated to OUTPUT_SUMMARY_MAX_CHARS), this returns
/// the complete text so the completed session isn't cut off mid-reply.
#[tauri::command]
pub fn read_session_output_cmd(session_id: String) -> Result<Option<String>, String> {
    Ok(pty::read_full_session_output(&session_id))
}

// Requirement commands
#[tauri::command]
pub fn load_requirements(db: State<'_, DbState>) -> Result<Vec<Requirement>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::requirement::load_requirements_from_db(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_requirement(db: State<'_, DbState>, req: Requirement) -> Result<(), String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::requirement::add_requirement_db(&conn, req).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_requirement(db: State<'_, DbState>, id: String, patch: serde_json::Value) -> Result<(), String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::requirement::update_requirement_db(&conn, &id, patch).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_requirement(db: State<'_, DbState>, id: String) -> Result<(), String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::requirement::remove_requirement_db(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_requirements_for_project(db: State<'_, DbState>, project_path: String) -> Result<Vec<Requirement>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::agents::requirement::get_requirements_for_project_db(&conn, &project_path).map_err(|e| e.to_string())
}

// Agent process lifecycle commands (PTY-based)
#[tauri::command]
pub fn spawn_agent_session(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
    db: State<'_, DbState>,
    project_path: String,
    agent_type: AgentType,
    prompt: String,
    model: Option<String>,
    linked_requirement_id: Option<String>,
    parent_session_id: Option<String>,
) -> Result<Session, String> {
    pty::spawn_pty_agent(
        &app,
        state.0.clone(),
        db.inner().clone(),
        &project_path,
        agent_type,
        &prompt,
        model.as_deref(),
        linked_requirement_id.as_deref(),
        parent_session_id.as_deref(),
    )
}

#[tauri::command]
pub fn stop_agent_session(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
    db: State<'_, DbState>,
    session_id: String,
) -> Result<(), String> {
    // Best-effort kill; process may already be dead (stale session)
    let _ = pty::stop_agent(&state.0, &session_id);

    // Always update session status so UI reflects the stop immediately
    let patch = serde_json::json!({
        "status": "failed",
        "finishedAt": chrono::Utc::now().to_rfc3339(),
        "exitCode": -1,
        "outputSummary": "Session stopped by user"
    });
    {
        let conn = db.get().map_err(|e| e.to_string())?;
        crate::agents::session::update_session_db(&conn, &session_id, patch).map_err(|e| e.to_string())?;
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
) -> Result<(), String> {
    pty::pty_write(&state.0, &session_id, &data)
}

#[tauri::command]
pub fn pty_resize_cmd(
    state: State<'_, AgentState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    pty::pty_resize(&state.0, &session_id, cols, rows)
}

// Activity commands
#[tauri::command]
pub fn get_project_activity(db: State<'_, DbState>, project_path: String) -> Result<Vec<crate::models::ActivityEvent>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::activity::get_events_for_project(&conn, &project_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_recent_activity(db: State<'_, DbState>, limit: Option<usize>) -> Result<Vec<crate::models::ActivityEvent>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::activity::get_recent_events(&conn, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

// Knowledge commands
#[tauri::command]
pub fn search_knowledge(db: State<'_, DbState>, query: String, limit: Option<usize>) -> Result<Vec<crate::models::KnowledgeEntry>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::knowledge::store::search_entries(&conn, &query, limit.unwrap_or(20)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_knowledge_for_project(db: State<'_, DbState>, project_path: String) -> Result<Vec<crate::models::KnowledgeEntry>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    let hash = crate::activity::hash_project_path(&project_path);
    crate::knowledge::store::get_entries_for_project(&conn, &hash).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_knowledge_entry(db: State<'_, DbState>, id: String) -> Result<(), String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::knowledge::store::delete_entry(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn trigger_knowledge_collection(
    db: State<'_, DbState>,
    project_path: String,
    session_id: String,
    agent_type: crate::models::AgentType,
) -> Result<usize, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::knowledge::collector::collect_from_session(&conn, &project_path, &session_id, &agent_type)
        .map_err(|e| e.to_string())
}

// Config commands
#[tauri::command]
pub fn load_mcp_config(project_path: String) -> Result<crate::models::McpConfigFile, String> {
    let path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    if !path.exists() {
        return Ok(crate::models::McpConfigFile { servers: vec![] });
    }
    crate::config::mcp::load_mcp_config(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_mcp_config(project_path: String, config: crate::models::McpConfigFile) -> Result<(), String> {
    let path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    crate::config::mcp::save_mcp_config(&config, &path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn apply_mcp_config(project_path: String, config: crate::models::McpConfigFile) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(&project_path);
    crate::config::adapters::apply_translations(&config, path).map_err(|e| e.to_string())
}

// Quality commands
#[tauri::command]
pub fn get_quality_reports(db: State<'_, DbState>) -> Result<Vec<crate::models::QualityReport>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::quality::report::get_all_reports(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_quality_report_for_session(db: State<'_, DbState>, session_id: String) -> Result<Option<crate::models::QualityReport>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    crate::quality::report::get_report_for_session(&conn, &session_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn run_quality_gate(project_path: String) -> Result<crate::models::QualityReport, String> {
    let path = std::path::Path::new(&project_path);
    crate::quality::forge::run_forge_gate(path).map_err(|e| e.to_string())
}
