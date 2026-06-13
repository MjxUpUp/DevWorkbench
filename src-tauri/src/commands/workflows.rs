//! Workflow management commands.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::models::Workflow;

#[tauri::command]
pub async fn list_workflows(db: State<'_, DbState>) -> Result<Vec<Workflow>, AppError> {
    let conn = db.0.lock().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    let mut stmt = conn.prepare(
        "SELECT id, name, yaml_content, created_at, updated_at FROM workflows ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Workflow {
            id: row.get(0)?,
            name: row.get(1)?,
            yaml_content: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    let mut workflows = Vec::new();
    for w in rows {
        workflows.push(w?);
    }
    Ok(workflows)
}

#[tauri::command]
pub async fn create_workflow(
    db: State<'_, DbState>,
    name: String,
    yaml_content: String,
) -> Result<Workflow, AppError> {
    let conn = db.0.lock().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO workflows (id, name, yaml_content, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, name, yaml_content, now, now],
    )?;
    Ok(Workflow {
        id,
        name,
        yaml_content,
        created_at: now.clone(),
        updated_at: now,
    })
}

#[tauri::command]
pub async fn get_workflow(db: State<'_, DbState>, id: String) -> Result<Workflow, AppError> {
    let conn = db.0.lock().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    conn.query_row(
        "SELECT id, name, yaml_content, created_at, updated_at FROM workflows WHERE id = ?1",
        [&id],
        |row| {
            Ok(Workflow {
                id: row.get(0)?,
                name: row.get(1)?,
                yaml_content: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .map_err(|e| AppError::NotFound(format!("Workflow not found: {}", e)))
}

#[tauri::command]
pub async fn update_workflow(
    db: State<'_, DbState>,
    id: String,
    name: String,
    yaml_content: String,
) -> Result<Workflow, AppError> {
    let conn = db.0.lock().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE workflows SET name = ?1, yaml_content = ?2, updated_at = ?3 WHERE id = ?4",
        rusqlite::params![name, yaml_content, now, id],
    )?;
    // Return the updated workflow
    conn.query_row(
        "SELECT id, name, yaml_content, created_at, updated_at FROM workflows WHERE id = ?1",
        [&id],
        |row| {
            Ok(Workflow {
                id: row.get(0)?,
                name: row.get(1)?,
                yaml_content: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .map_err(|e| AppError::NotFound(format!("Workflow not found after update: {}", e)))
}

#[tauri::command]
pub async fn delete_workflow(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.0.lock().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    conn.execute("DELETE FROM workflows WHERE id = ?1", [&id])?;
    Ok(())
}

// Note: run_workflow was a stub returning "not yet implemented".
// It has been removed — the real execution engine will land in Phase 1 as
// kernel-compose::Graph, exposed via a new command surface that streams
// AgentEvent rather than returning a single String.
