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

/// Create a user-defined slash command. `name` carries no leading slash and
/// must be unique (builtins are seeded separately). Closes the dive_02 gap: the
/// UI can now let users AUTHOR their own `/` commands, not just consume builtins.
#[tauri::command]
pub async fn create_slash_command(
    db: State<'_, DbState>,
    name: String,
    description: Option<String>,
    template: String,
    category: Option<String>,
) -> Result<SlashCommand, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::slash_commands::registry::create_command(
        &conn,
        &name,
        description.as_deref(),
        &template,
        category.as_deref(),
    )
}

/// Update a user command's fields by id. Built-ins (category=builtin) are
/// protected server-side — the call errors instead of mutating the seeded baseline.
#[tauri::command]
pub async fn update_slash_command(
    db: State<'_, DbState>,
    id: String,
    name: String,
    description: Option<String>,
    template: String,
    category: Option<String>,
) -> Result<(), AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::slash_commands::registry::update_command(
        &conn,
        &id,
        &name,
        description.as_deref(),
        &template,
        category.as_deref(),
    )
}

/// Delete a user command by id. Built-ins are protected server-side.
#[tauri::command]
pub async fn delete_slash_command(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::slash_commands::registry::delete_command(&conn, &id)
}
