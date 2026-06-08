use crate::models::{Requirement, Session};

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
