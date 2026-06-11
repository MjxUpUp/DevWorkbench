use crate::error::AppError;
use crate::models::{AgentType, ContextSnapshot, Session, SessionStatus};
use rusqlite::params;
use std::path::PathBuf;

// Thread-local override for database path (test isolation).
#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_DB_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Get a connection for tests (bypasses Tauri state).
#[cfg(test)]
fn test_conn() -> rusqlite::Connection {
    TEST_DB_PATH_OVERRIDE.with(|cell| {
        let path = cell.borrow().clone().expect("TEST_DB_PATH_OVERRIDE not set — use TempDb guard");
        rusqlite::Connection::open(&path).expect("failed to open test DB")
    })
}

pub fn load_sessions_from_db(conn: &rusqlite::Connection) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_path, agent_type, status, prompt, model,
                started_at, finished_at, exit_code, output_summary,
                context_snapshot, linked_requirement_id, parent_session_id
         FROM sessions ORDER BY started_at DESC"
    )?;

    let sessions = stmt.query_map([], |row| {
        let agent_type_str: String = row.get(2)?;
        let agent_type: AgentType = serde_json::from_value(serde_json::Value::String(agent_type_str))
            .unwrap_or(AgentType::ClaudeCode);

        let status_str: String = row.get(3)?;
        let status = match status_str.as_str() {
            "running" => SessionStatus::Running,
            "completed" => SessionStatus::Completed,
            "failed" => SessionStatus::Failed,
            _ => SessionStatus::Failed,
        };

        let snapshot_str: Option<String> = row.get(10)?;
        let context_snapshot: Option<ContextSnapshot> = snapshot_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(Session {
            id: row.get(0)?,
            project_path: row.get(1)?,
            agent_type,
            status,
            prompt: row.get(4)?,
            model: row.get(5)?,
            started_at: row.get(6)?,
            finished_at: row.get(7)?,
            exit_code: row.get(8)?,
            output_summary: row.get(9)?,
            context_snapshot,
            linked_requirement_id: row.get(11)?,
            parent_session_id: row.get(12)?,
        })
    })?;

    let mut result = Vec::new();
    for s in sessions {
        result.push(s?);
    }

    // Reconcile stale running sessions
    let mut dirty = false;
    let now = chrono::Local::now();
    for s in &mut result {
        if s.status == SessionStatus::Running {
            if let Ok(started) = chrono::DateTime::parse_from_rfc3339(&s.started_at) {
                let started_local = started.with_timezone(&chrono::Local);
                if (now - started_local).num_minutes() > 10 {
                    s.status = SessionStatus::Failed;
                    s.finished_at = Some(now.to_rfc3339());
                    s.exit_code = Some(-1);
                    s.output_summary = Some("Session was interrupted (app restart)".to_string());
                    dirty = true;
                }
            }
        }
    }
    if dirty {
        // Update stale sessions in DB
        for s in &result {
            if s.status == SessionStatus::Failed && s.exit_code == Some(-1) {
                let _ = conn.execute(
                    "UPDATE sessions SET status = 'failed', finished_at = ?1, exit_code = -1, output_summary = 'Session was interrupted (app restart)' WHERE id = ?2",
                    params![s.finished_at, s.id],
                );
            }
        }
    }

    Ok(result)
}

pub fn insert_session_db(conn: &rusqlite::Connection, s: &Session) -> Result<(), AppError> {
    let snapshot_json = s.context_snapshot.as_ref().map(|cs| serde_json::to_string(cs).unwrap_or_default());
    conn.execute(
        "INSERT OR IGNORE INTO sessions
            (id, project_path, agent_type, status, prompt, model,
             started_at, finished_at, exit_code, output_summary,
             context_snapshot, linked_requirement_id, parent_session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            s.id,
            s.project_path,
            serde_json::to_string(&s.agent_type)?.trim_matches('"'),
            s.status.as_str(),
            s.prompt,
            s.model,
            s.started_at,
            s.finished_at,
            s.exit_code,
            s.output_summary,
            snapshot_json,
            s.linked_requirement_id,
            s.parent_session_id,
        ],
    )?;
    Ok(())
}

pub fn update_session_db(conn: &rusqlite::Connection, id: &str, patch: serde_json::Value) -> Result<(), AppError> {
    // Build SET clause dynamically based on provided fields
    let mut set_clauses: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
        let validated = match status {
            "running" => "running",
            "completed" => "completed",
            "failed" => "failed",
            _ => return Err(AppError::Agent(format!("无效 status: {}", status))),
        };
        set_clauses.push("status = ?".to_string());
        param_values.push(Box::new(validated.to_string()));
    }
    if let Some(exit_code) = patch.get("exitCode").or_else(|| patch.get("exit_code")).and_then(|v| v.as_i64()) {
        set_clauses.push("exit_code = ?".to_string());
        param_values.push(Box::new(exit_code as i32));
    }
    if let Some(finished_at) = patch.get("finishedAt").or_else(|| patch.get("finished_at")).and_then(|v| v.as_str()) {
        set_clauses.push("finished_at = ?".to_string());
        param_values.push(Box::new(finished_at.to_string()));
    }
    if let Some(summary) = patch.get("outputSummary").or_else(|| patch.get("output_summary")).and_then(|v| v.as_str()) {
        set_clauses.push("output_summary = ?".to_string());
        param_values.push(Box::new(summary.to_string()));
    }
    if let Some(snap) = patch.get("contextSnapshot").or_else(|| patch.get("context_snapshot")) {
        let snap_json = serde_json::to_string(snap).unwrap_or_default();
        set_clauses.push("context_snapshot = ?".to_string());
        param_values.push(Box::new(snap_json));
    }

    if set_clauses.is_empty() {
        return Ok(());
    }

    let sql = format!("UPDATE sessions SET {} WHERE id = ?", set_clauses.join(", "));
    param_values.push(Box::new(id.to_string()));

    let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let rows = conn.execute(&sql, params.as_slice())?;
    if rows == 0 {
        return Err(AppError::NotFound(format!("Session {} 不存在", id)));
    }
    Ok(())
}

pub fn get_sessions_for_project_db(conn: &rusqlite::Connection, project_path: &str) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_path, agent_type, status, prompt, model,
                started_at, finished_at, exit_code, output_summary,
                context_snapshot, linked_requirement_id, parent_session_id
         FROM sessions WHERE project_path = ?1 ORDER BY started_at DESC"
    )?;

    let sessions = stmt.query_map(params![project_path], |row| {
        let agent_type_str: String = row.get(2)?;
        let agent_type: AgentType = serde_json::from_value(serde_json::Value::String(agent_type_str))
            .unwrap_or(AgentType::ClaudeCode);

        let status_str: String = row.get(3)?;
        let status = match status_str.as_str() {
            "running" => SessionStatus::Running,
            "completed" => SessionStatus::Completed,
            "failed" => SessionStatus::Failed,
            _ => SessionStatus::Failed,
        };

        let snapshot_str: Option<String> = row.get(10)?;
        let context_snapshot: Option<ContextSnapshot> = snapshot_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(Session {
            id: row.get(0)?,
            project_path: row.get(1)?,
            agent_type,
            status,
            prompt: row.get(4)?,
            model: row.get(5)?,
            started_at: row.get(6)?,
            finished_at: row.get(7)?,
            exit_code: row.get(8)?,
            output_summary: row.get(9)?,
            context_snapshot,
            linked_requirement_id: row.get(11)?,
            parent_session_id: row.get(12)?,
        })
    })?;

    let mut result = Vec::new();
    for s in sessions {
        result.push(s?);
    }
    Ok(result)
}

// ---- Legacy helpers (still used by pty output logging) ----

pub(crate) fn agents_dir() -> Result<PathBuf, String> {
    let home = crate::commands::projects::dirs_home();
    let dir = home.join(".dev-workbench").join("agents");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建 agents 目录失败: {}", e))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::SessionStatus;

    /// RAII guard: creates a temp SQLite DB and sets thread-local override.
    struct TempDb {
        _tmp: tempfile::TempDir,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let _conn = db::init_db(&db_path).expect("init_db failed");
            // Drop the connection so test_conn can open it
            drop(_conn);
            TEST_DB_PATH_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(db_path));
            Self { _tmp: tmp }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            TEST_DB_PATH_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
        }
    }

    fn make_session(id: &str, project: &str, status: SessionStatus) -> Session {
        Session {
            id: id.to_string(),
            project_path: project.to_string(),
            agent_type: AgentType::ClaudeCode,
            status,
            prompt: "test prompt".to_string(),
            model: None,
            started_at: chrono::Local::now().to_rfc3339(),
            finished_at: None,
            exit_code: None,
            output_summary: None,
            context_snapshot: None,
            linked_requirement_id: None,
            parent_session_id: None,
        }
    }

    #[test]
    fn test_add_and_load_sessions() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();
        insert_session_db(&conn, &make_session("s2", "/proj/b", SessionStatus::Completed)).unwrap();

        let loaded = load_sessions_from_db(&conn).unwrap();
        assert_eq!(loaded.len(), 2);
        let ids: Vec<&str> = loaded.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"s1"));
        assert!(ids.contains(&"s2"));
    }

    #[test]
    fn test_update_session_status() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        let patch = serde_json::json!({ "status": "completed", "exitCode": 0 });
        update_session_db(&conn, "s1", patch).unwrap();

        let loaded = load_sessions_from_db(&conn).unwrap();
        assert_eq!(loaded[0].status, SessionStatus::Completed);
        assert_eq!(loaded[0].exit_code, Some(0));
    }

    #[test]
    fn test_get_sessions_for_project() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();
        insert_session_db(&conn, &make_session("s2", "/proj/b", SessionStatus::Completed)).unwrap();
        insert_session_db(&conn, &make_session("s3", "/proj/a", SessionStatus::Completed)).unwrap();

        let proj_a = get_sessions_for_project_db(&conn, "/proj/a").unwrap();
        assert_eq!(proj_a.len(), 2);
        assert!(proj_a.iter().all(|s| s.project_path == "/proj/a"));
    }

    #[test]
    fn test_update_session_invalid_status() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        let patch = serde_json::json!({ "status": "invalid_status" });
        let result = update_session_db(&conn, "s1", patch);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("无效 status"));
    }
}
