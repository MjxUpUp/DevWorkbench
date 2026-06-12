//! Skills management commands.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::models::Skill;

#[tauri::command]
pub async fn list_skills(db: State<'_, DbState>) -> Result<Vec<Skill>, AppError> {
    let conn = db.0.lock().map_err(|e| crate::error::AppError::Config(format!("Lock error: {}", e)))?;
    crate::skills::registry::list_skills(&conn)
}

#[tauri::command]
pub async fn install_skill(db: State<'_, DbState>, skill: Skill) -> Result<(), AppError> {
    let conn = db.0.lock().map_err(|e| crate::error::AppError::Config(format!("Lock error: {}", e)))?;
    crate::skills::registry::install_skill(&conn, &skill)
}

#[tauri::command]
pub async fn uninstall_skill(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.0.lock().map_err(|e| crate::error::AppError::Config(format!("Lock error: {}", e)))?;
    crate::skills::registry::uninstall_skill(&conn, &id)
}
