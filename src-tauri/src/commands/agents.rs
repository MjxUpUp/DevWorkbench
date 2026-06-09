use crate::agents::discovery::{discover_agents, recommend_agent, AgentInfo};
use crate::agents::pty;
use crate::models::{AgentType, Requirement, Session};
use std::sync::Arc;
use tauri::State;

/// Tauri managed state wrapping AgentProcesses (PTY-based)
pub struct AgentState(pub Arc<pty::AgentProcesses>);

// Agent discovery commands
#[tauri::command]
pub fn discover_agents_cmd() -> Result<Vec<AgentInfo>, String> {
    Ok(discover_agents())
}

#[tauri::command]
pub fn recommend_agent_for_project(tags: Vec<String>) -> Result<Option<AgentType>, String> {
    Ok(recommend_agent(&tags))
}

// Session commands
#[tauri::command]
pub fn load_sessions() -> Result<Vec<Session>, String> {
    crate::agents::session::load_sessions()
}

#[tauri::command]
pub fn get_sessions_for_project(project_path: String) -> Result<Vec<Session>, String> {
    crate::agents::session::get_sessions_for_project(&project_path)
}

#[tauri::command]
pub fn update_session(id: String, patch: serde_json::Value) -> Result<Vec<Session>, String> {
    crate::agents::session::update_session(&id, patch)
}

// Requirement commands
#[tauri::command]
pub fn load_requirements() -> Result<Vec<Requirement>, String> {
    crate::agents::requirement::load_requirements()
}

#[tauri::command]
pub fn add_requirement(req: Requirement) -> Result<Vec<Requirement>, String> {
    crate::agents::requirement::add_requirement(req)
}

#[tauri::command]
pub fn update_requirement(id: String, patch: serde_json::Value) -> Result<Vec<Requirement>, String> {
    crate::agents::requirement::update_requirement(&id, patch)
}

#[tauri::command]
pub fn remove_requirement(id: String) -> Result<Vec<Requirement>, String> {
    crate::agents::requirement::remove_requirement(&id)
}

#[tauri::command]
pub fn get_requirements_for_project(project_path: String) -> Result<Vec<Requirement>, String> {
    crate::agents::requirement::get_requirements_for_project(&project_path)
}

// Agent process lifecycle commands (PTY-based)
#[tauri::command]
pub fn spawn_agent_session(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
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
    state: State<'_, AgentState>,
    session_id: String,
) -> Result<(), String> {
    pty::stop_agent(&state.0, &session_id)
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
