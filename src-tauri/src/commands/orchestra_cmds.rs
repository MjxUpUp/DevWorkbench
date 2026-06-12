//! Orchestra (OwnAgent) management commands.

use tauri::State;

use crate::error::AppError;
use crate::orchestra::sidecar::OwnAgentSidecar;

#[tauri::command]
pub async fn orchestra_start(sidecar: State<'_, OwnAgentSidecar>) -> Result<(), AppError> {
    sidecar.start()
}

#[tauri::command]
pub async fn orchestra_stop(sidecar: State<'_, OwnAgentSidecar>) -> Result<(), AppError> {
    sidecar.stop()
}

#[tauri::command]
pub async fn orchestra_status(sidecar: State<'_, OwnAgentSidecar>) -> Result<bool, AppError> {
    Ok(sidecar.is_running())
}
