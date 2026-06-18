//! User hook commands — CRUD + toggle for the D2 lifecycle-hook config layer.
//! Mirrors `slash_cmds.rs`: thin Tauri wrappers over `user_hooks::registry`.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::models::{UserHook, UserHookEvent};

/// List every user hook (enabled + disabled), ordered by name. The settings UI
/// reads this to render the management list.
#[tauri::command]
pub async fn list_user_hooks(db: State<'_, DbState>) -> Result<Vec<UserHook>, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::user_hooks::registry::list_user_hooks(&conn)
}

/// Create a user hook. `name` must be unique. `matcher` scopes a tool-event hook
/// to specific tools (claude-code `matcher`); None/empty = match all.
#[tauri::command]
pub async fn create_user_hook(
    db: State<'_, DbState>,
    name: String,
    event: UserHookEvent,
    command: String,
    shell: Option<bool>,
    timeout_secs: Option<u64>,
    enabled: Option<bool>,
    matcher: Option<String>,
) -> Result<UserHook, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::user_hooks::registry::create_hook(
        &conn,
        &name,
        event,
        &command,
        shell.unwrap_or(true),
        timeout_secs.unwrap_or(30),
        enabled.unwrap_or(true),
        matcher.as_deref(),
    )
}

/// Update a user hook's editable fields by id.
#[tauri::command]
pub async fn update_user_hook(
    db: State<'_, DbState>,
    id: String,
    name: String,
    event: UserHookEvent,
    command: String,
    shell: bool,
    timeout_secs: u64,
    enabled: bool,
    matcher: Option<String>,
) -> Result<(), AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::user_hooks::registry::update_hook(
        &conn, &id, &name, event, &command, shell, timeout_secs, enabled, matcher.as_deref(),
    )
}

/// Flip a hook's enabled flag without re-POSTing the whole row (the list-card
/// toggle calls this).
#[tauri::command]
pub async fn set_user_hook_enabled(
    db: State<'_, DbState>,
    id: String,
    enabled: bool,
) -> Result<(), AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::user_hooks::registry::set_enabled(&conn, &id, enabled)
}

/// Delete a user hook by id.
#[tauri::command]
pub async fn delete_user_hook(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::user_hooks::registry::delete_hook(&conn, &id)
}
