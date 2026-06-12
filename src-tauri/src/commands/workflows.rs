//! Workflow management commands.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::models::Workflow;

#[tauri::command]
pub async fn list_workflows(_db: State<'_, DbState>) -> Result<Vec<Workflow>, AppError> {
    // Skeleton — full implementation to follow.
    Ok(Vec::new())
}

#[tauri::command]
pub async fn create_workflow(_db: State<'_, DbState>, _name: String, _yaml_content: String) -> Result<Workflow, AppError> {
    // Skeleton — full implementation to follow.
    Err(AppError::NotFound("create_workflow not yet implemented".into()))
}

#[tauri::command]
pub async fn run_workflow(_db: State<'_, DbState>, _workflow_id: String) -> Result<String, AppError> {
    // Skeleton — full implementation to follow.
    Err(AppError::NotFound("run_workflow not yet implemented".into()))
}
