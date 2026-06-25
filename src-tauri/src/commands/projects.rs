use crate::db::DbState;
use crate::models::{AppSettings, Project};
use rusqlite::params;
use std::path::PathBuf;
use tauri::State;

pub fn dirs_home() -> PathBuf {
    // Windows 上 USERPROFILE 始终是原生路径（C:\Users\xxx），
    // 而 HOME 可能是 Git Bash 设置的 Unix 风格路径（/c/Users/xxx），
    // PathBuf 无法正确解析后者。所以 Windows 上优先用 USERPROFILE。
    #[cfg(target_os = "windows")]
    {
        if let Ok(home) = std::env::var("USERPROFILE") {
            return PathBuf::from(home);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home);
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home);
    }
    PathBuf::from(".")
}

// ── Row mapping helpers ──

fn row_to_project(row: &rusqlite::Row<'_>) -> Result<Project, rusqlite::Error> {
    let tags_str: String = row.get(4)?;
    let lot_str: String = row.get(10)?;
    let wt_str: String = row.get(11)?;
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        path: row.get(3)?,
        tags: serde_json::from_str(&tags_str).unwrap_or_default(),
        cover_image: row.get(5)?,
        open_count: row.get(6)?,
        last_opened_at: row.get(7)?,
        starred: row.get::<_, i32>(8)? != 0,
        created_at: row.get(9)?,
        last_opened_tools: serde_json::from_str(&lot_str).unwrap_or_default(),
        workspace_tools: serde_json::from_str(&wt_str).unwrap_or_default(),
    })
}

const PROJECT_COLUMNS: &str = "\
    id, name, description, path, tags, cover_image, open_count, \
    last_opened_at, starred, created_at, last_opened_tools, workspace_tools";

fn load_all_projects(conn: &rusqlite::Connection) -> Result<Vec<Project>, String> {
    let sql = format!(
        "SELECT {} FROM projects ORDER BY created_at DESC",
        PROJECT_COLUMNS
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_project)
        .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for p in rows {
        result.push(p.map_err(|e| e.to_string())?);
    }
    Ok(result)
}

// ── Project commands ──

#[tauri::command]
pub fn load_projects(db: State<'_, DbState>) -> Result<Vec<Project>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    load_all_projects(&conn)
}

#[tauri::command]
pub fn add_project(db: State<'_, DbState>, project: Project) -> Result<Vec<Project>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO projects ({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            PROJECT_COLUMNS
        ),
        params![
            project.id,
            project.name,
            project.description,
            project.path,
            serde_json::to_string(&project.tags).map_err(|e| e.to_string())?,
            project.cover_image,
            project.open_count,
            project.last_opened_at,
            project.starred as i32,
            project.created_at,
            serde_json::to_string(&project.last_opened_tools).map_err(|e| e.to_string())?,
            serde_json::to_string(&project.workspace_tools).map_err(|e| e.to_string())?,
        ],
    )
    .map_err(|e| e.to_string())?;
    load_all_projects(&conn)
}

#[tauri::command]
pub fn remove_project(db: State<'_, DbState>, id: String) -> Result<Vec<Project>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    let rows = conn
        .execute("DELETE FROM projects WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    if rows == 0 {
        return Err(format!("项目 {} 不存在", id));
    }
    load_all_projects(&conn)
}

#[tauri::command]
pub fn update_project(
    db: State<'_, DbState>,
    id: String,
    patch: serde_json::Value,
) -> Result<Vec<Project>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;

    let mut set_clauses: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(v) = patch.get("name").and_then(|v| v.as_str()) {
        set_clauses.push("name = ?".into());
        param_values.push(Box::new(v.to_string()));
    }
    if let Some(v) = patch.get("description").and_then(|v| v.as_str()) {
        set_clauses.push("description = ?".into());
        param_values.push(Box::new(v.to_string()));
    }
    if let Some(v) = patch.get("path").and_then(|v| v.as_str()) {
        set_clauses.push("path = ?".into());
        param_values.push(Box::new(v.to_string()));
    }
    if let Some(arr) = patch.get("tags").and_then(|v| v.as_array()) {
        let tags: Vec<String> = arr
            .iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
        set_clauses.push("tags = ?".into());
        param_values.push(Box::new(
            serde_json::to_string(&tags).map_err(|e| e.to_string())?,
        ));
    }
    if let Some(v) = patch.get("coverImage").or_else(|| patch.get("cover_image")) {
        let val: Option<String> = v.as_str().map(String::from);
        set_clauses.push("cover_image = ?".into());
        param_values.push(Box::new(val));
    }
    if let Some(v) = patch.get("starred").and_then(|v| v.as_bool()) {
        set_clauses.push("starred = ?".into());
        param_values.push(Box::new(v as i32));
    }
    if let Some(arr) = patch
        .get("workspaceTools")
        .or_else(|| patch.get("workspace_tools"))
        .and_then(serde_json::Value::as_array)
    {
        let tools: Vec<String> = arr
            .iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
        set_clauses.push("workspace_tools = ?".into());
        param_values.push(Box::new(
            serde_json::to_string(&tools).map_err(|e| e.to_string())?,
        ));
    }

    if !set_clauses.is_empty() {
        let sql = format!(
            "UPDATE projects SET {} WHERE id = ?",
            set_clauses.join(", ")
        );
        param_values.push(Box::new(id.clone()));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let rows = conn
            .execute(&sql, param_refs.as_slice())
            .map_err(|e| e.to_string())?;
        if rows == 0 {
            return Err(format!("项目 {} 不存在", id));
        }
    }

    load_all_projects(&conn)
}

#[tauri::command]
pub fn update_project_open(db: State<'_, DbState>, id: String) -> Result<Vec<Project>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "UPDATE projects SET open_count = open_count + 1, last_opened_at = ?1 WHERE id = ?2",
        params![now, id],
    )
    .map_err(|e| e.to_string())?;
    load_all_projects(&conn)
}

#[tauri::command]
pub fn record_tool_open(
    db: State<'_, DbState>,
    id: String,
    tool_name: String,
) -> Result<Vec<Project>, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    // Serialize the read-modify-write of last_opened_tools in a transaction so
    // two concurrent record_tool_open calls can't both read the stale list and
    // clobber each other's update (lost-write TOCTOU). unchecked_transaction
    // matches the &Connection borrow; the guard rolls back on any error path.
    {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let tools_json: String = tx
            .query_row(
                "SELECT last_opened_tools FROM projects WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|e| format!("项目 {} 不存在: {}", id, e))?;

        let mut tools: Vec<String> = serde_json::from_str(&tools_json).unwrap_or_default();
        tools.retain(|t| t != &tool_name);
        tools.insert(0, tool_name);
        tools.truncate(5);

        tx.execute(
            "UPDATE projects SET last_opened_tools = ?1 WHERE id = ?2",
            params![
                serde_json::to_string(&tools).map_err(|e| e.to_string())?,
                id
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }

    load_all_projects(&conn)
}

// ── Settings commands ──

/// Internal settings loader — can be called from other modules with a Connection.
pub fn load_settings_from_db(conn: &rusqlite::Connection) -> Result<AppSettings, String> {
    let result = conn.query_row(
        "SELECT scan_directories, tool_paths, theme, palette, preferred_terminal, cli_flags FROM settings WHERE id = 1",
        [],
        |row| {
            let sd: String = row.get(0)?;
            let tp: String = row.get(1)?;
            // palette (col 3) is nullable on DBs upgraded from v18→v19 (ALTER
            // COLUMN with no NOT NULL); fall back to the default so a NULL row
            // still yields AppSettings.palette == "pi".
            let palette: Option<String> = row.get(3)?;
            let cf: String = row.get(5)?;
            Ok(AppSettings {
                scan_directories: serde_json::from_str(&sd).unwrap_or_default(),
                tool_paths: serde_json::from_str(&tp).unwrap_or_default(),
                theme: row.get(2)?,
                palette: palette.unwrap_or_else(|| "pi".to_string()),
                preferred_terminal: row.get(4)?,
                cli_flags: serde_json::from_str(&cf).unwrap_or_default(),
            })
        },
    );
    match result {
        Ok(s) => Ok(s),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            let defaults = AppSettings {
                scan_directories: Vec::new(),
                tool_paths: std::collections::HashMap::new(),
                theme: "obsidian".to_string(),
                palette: "pi".to_string(),
                preferred_terminal: String::new(),
                cli_flags: std::collections::HashMap::new(),
            };
            conn.execute(
                "INSERT OR IGNORE INTO settings (id, scan_directories, tool_paths, theme, palette, preferred_terminal, cli_flags) VALUES (1, '[]', '{}', 'auto', 'pi', '', '{}')",
                [],
            ).map_err(|e| e.to_string())?;
            Ok(defaults)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Internal settings saver — can be called from other modules with a Connection.
pub fn save_settings_to_db(
    conn: &rusqlite::Connection,
    settings: &AppSettings,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (id, scan_directories, tool_paths, theme, palette, preferred_terminal, cli_flags) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            serde_json::to_string(&settings.scan_directories).map_err(|e| e.to_string())?,
            serde_json::to_string(&settings.tool_paths).map_err(|e| e.to_string())?,
            settings.theme,
            settings.palette,
            settings.preferred_terminal,
            serde_json::to_string(&settings.cli_flags).map_err(|e| e.to_string())?,
        ],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn load_settings(db: State<'_, DbState>) -> Result<AppSettings, String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    load_settings_from_db(&conn)
}

#[tauri::command]
pub fn save_settings(db: State<'_, DbState>, settings: AppSettings) -> Result<(), String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    save_settings_to_db(&conn, &settings)
}
