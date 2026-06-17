//! Slash command commands — list + render.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::models::SlashCommand;

/// List every slash command (built-in + user-defined). The `/` trigger menu
/// reads this instead of a hardcoded frontend list.
#[tauri::command]
pub async fn list_slash_commands(db: State<'_, DbState>) -> Result<Vec<SlashCommand>, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::slash_commands::registry::list_slash_commands(&conn)
}

/// Render a named command's template with the given arguments. The frontend
/// can call this to preview the expanded prompt; spawn_agent_session also
/// renders inline at submit time so the kernel always sees the expanded text.
#[tauri::command]
pub async fn render_slash_command(
    db: State<'_, DbState>,
    name: String,
    arguments: String,
) -> Result<String, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    match crate::slash_commands::registry::find_by_name(&conn, &name)? {
        Some(cmd) => Ok(crate::slash_commands::registry::render_template(
            &cmd.template,
            &arguments,
        )),
        None => Err(AppError::Config(format!("unknown slash command: /{name}"))),
    }
}
