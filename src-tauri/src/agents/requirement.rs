use crate::error::AppError;
use crate::models::{Requirement, RequirementStatus};
use rusqlite::params;

pub fn load_requirements_from_db(conn: &rusqlite::Connection) -> Result<Vec<Requirement>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_path, title, description, status, priority,
                linked_session_id, artifacts, created_at, updated_at
         FROM requirements ORDER BY updated_at DESC"
    )?;

    let reqs = stmt.query_map([], |row| {
        let status_str: String = row.get(4)?;
        let status = match status_str.as_str() {
            "todo" => RequirementStatus::Todo,
            "in_progress" => RequirementStatus::InProgress,
            "done" => RequirementStatus::Done,
            _ => RequirementStatus::Todo,
        };

        let artifacts_str: String = row.get(7)?;
        let artifacts: Vec<String> = serde_json::from_str(&artifacts_str).unwrap_or_default();

        Ok(Requirement {
            id: row.get(0)?,
            project_path: row.get(1)?,
            title: row.get(2)?,
            description: row.get(3)?,
            status,
            priority: row.get(5)?,
            linked_session_id: row.get(6)?,
            artifacts,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    })?;

    let mut result = Vec::new();
    for r in reqs {
        result.push(r?);
    }

    // Reconcile: fix requirements stuck in in_progress when their linked
    // session has already completed or failed
    let mut dirty = false;
    for req in &mut result {
        if req.status != RequirementStatus::InProgress {
            continue;
        }
        let Some(ref sid) = req.linked_session_id else { continue };

        let session_status: Option<String> = conn.query_row(
            "SELECT status FROM sessions WHERE id = ?1",
            params![sid],
            |row| row.get(0),
        ).ok();

        let Some(status_str) = session_status else { continue };
        match status_str.as_str() {
            "completed" => {
                req.status = RequirementStatus::Done;
                req.updated_at = chrono::Local::now().to_rfc3339();
                dirty = true;
            }
            "failed" => {
                req.status = RequirementStatus::Todo;
                req.linked_session_id = None;
                req.updated_at = chrono::Local::now().to_rfc3339();
                dirty = true;
            }
            _ => {}
        }
    }
    if dirty {
        for req in &result {
            let _ = conn.execute(
                "UPDATE requirements SET status = ?1, linked_session_id = ?2, updated_at = ?3 WHERE id = ?4",
                params![req.status.as_str(), req.linked_session_id, req.updated_at, req.id],
            );
        }
    }

    Ok(result)
}

pub fn add_requirement_db(conn: &rusqlite::Connection, mut req: Requirement) -> Result<(), AppError> {
    if req.id.is_empty() {
        req.id = uuid::Uuid::new_v4().to_string();
    }
    let now = chrono::Local::now().to_rfc3339();
    if req.created_at.is_empty() {
        req.created_at = now.clone();
    }
    req.updated_at = now;

    let artifacts_json = serde_json::to_string(&req.artifacts).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT OR IGNORE INTO requirements
            (id, project_path, title, description, status, priority,
             linked_session_id, artifacts, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            req.id,
            req.project_path,
            req.title,
            req.description,
            req.status.as_str(),
            req.priority,
            req.linked_session_id,
            artifacts_json,
            req.created_at,
            req.updated_at,
        ],
    )?;
    Ok(())
}

pub fn update_requirement_db(conn: &rusqlite::Connection, id: &str, patch: serde_json::Value) -> Result<(), AppError> {
    let mut set_clauses: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(title) = patch.get("title").and_then(|v| v.as_str()) {
        set_clauses.push("title = ?".to_string());
        param_values.push(Box::new(title.to_string()));
    }
    if let Some(desc) = patch.get("description") {
        let desc_str = desc.as_str().map(|s| s.to_string());
        set_clauses.push("description = ?".to_string());
        param_values.push(Box::new(desc_str));
    }
    if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
        let validated = match status {
            "todo" => "todo",
            "in_progress" => "in_progress",
            "done" => "done",
            _ => return Err(AppError::Agent(format!("无效 status: {}", status))),
        };
        set_clauses.push("status = ?".to_string());
        param_values.push(Box::new(validated.to_string()));
    }
    if let Some(priority) = patch.get("priority") {
        let p = priority.as_str().map(|s| s.to_string());
        set_clauses.push("priority = ?".to_string());
        param_values.push(Box::new(p));
    }
    if let Some(session_id) = patch.get("linkedSessionId") {
        let sid = session_id.as_str().map(|s| s.to_string());
        set_clauses.push("linked_session_id = ?".to_string());
        param_values.push(Box::new(sid));
    }
    if let Some(artifacts) = patch.get("artifacts") {
        let arts = serde_json::to_string(artifacts).unwrap_or_else(|_| "[]".to_string());
        set_clauses.push("artifacts = ?".to_string());
        param_values.push(Box::new(arts));
    }

    // Always update updated_at
    let now = chrono::Local::now().to_rfc3339();
    set_clauses.push("updated_at = ?".to_string());
    param_values.push(Box::new(now));

    if set_clauses.len() == 1 {
        // Only updated_at, nothing else to change
        // Still execute to touch the timestamp
    }

    let sql = format!("UPDATE requirements SET {} WHERE id = ?", set_clauses.join(", "));
    param_values.push(Box::new(id.to_string()));

    let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let rows = conn.execute(&sql, params.as_slice())?;
    if rows == 0 {
        return Err(AppError::NotFound(format!("Requirement {} 不存在", id)));
    }
    Ok(())
}

pub fn remove_requirement_db(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    let rows = conn.execute("DELETE FROM requirements WHERE id = ?1", params![id])?;
    if rows == 0 {
        return Err(AppError::NotFound(format!("Requirement {} 不存在", id)));
    }
    Ok(())
}

pub fn get_requirements_for_project_db(conn: &rusqlite::Connection, project_path: &str) -> Result<Vec<Requirement>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_path, title, description, status, priority,
                linked_session_id, artifacts, created_at, updated_at
         FROM requirements WHERE project_path = ?1 ORDER BY updated_at DESC"
    )?;

    let reqs = stmt.query_map(params![project_path], |row| {
        let status_str: String = row.get(4)?;
        let status = match status_str.as_str() {
            "todo" => RequirementStatus::Todo,
            "in_progress" => RequirementStatus::InProgress,
            "done" => RequirementStatus::Done,
            _ => RequirementStatus::Todo,
        };

        let artifacts_str: String = row.get(7)?;
        let artifacts: Vec<String> = serde_json::from_str(&artifacts_str).unwrap_or_default();

        Ok(Requirement {
            id: row.get(0)?,
            project_path: row.get(1)?,
            title: row.get(2)?,
            description: row.get(3)?,
            status,
            priority: row.get(5)?,
            linked_session_id: row.get(6)?,
            artifacts,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    })?;

    let mut result = Vec::new();
    for r in reqs {
        result.push(r?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::RequirementStatus;

    /// RAII guard: creates a temp SQLite DB.
    struct TempDb {
        _tmp: tempfile::TempDir,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let _conn = db::init_db(&db_path).expect("init_db failed");
            drop(_conn);
            super::super::session::TEST_DB_PATH_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(db_path));
            Self { _tmp: tmp }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            super::super::session::TEST_DB_PATH_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
        }
    }

    fn test_conn() -> rusqlite::Connection {
        super::super::session::TEST_DB_PATH_OVERRIDE.with(|cell| {
            let path = cell.borrow().clone().expect("TEST_DB_PATH_OVERRIDE not set");
            rusqlite::Connection::open(&path).expect("failed to open test DB")
        })
    }

    fn make_requirement(id: &str, project: &str, status: RequirementStatus) -> Requirement {
        Requirement {
            id: id.to_string(),
            project_path: project.to_string(),
            title: format!("Requirement {}", id),
            description: None,
            status,
            priority: None,
            linked_session_id: None,
            artifacts: vec![],
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
        }
    }

    #[test]
    fn test_add_and_load_requirements() {
        let _guard = TempDb::new();
        let conn = test_conn();

        add_requirement_db(&conn, make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();
        add_requirement_db(&conn, make_requirement("r2", "/proj/b", RequirementStatus::InProgress)).unwrap();

        let loaded = load_requirements_from_db(&conn).unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn test_update_requirement_status_transitions() {
        let _guard = TempDb::new();
        let conn = test_conn();

        add_requirement_db(&conn, make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();

        // Todo -> InProgress
        let patch = serde_json::json!({ "status": "in_progress" });
        update_requirement_db(&conn, "r1", patch).unwrap();

        let loaded = load_requirements_from_db(&conn).unwrap();
        assert_eq!(loaded[0].status, RequirementStatus::InProgress);

        // InProgress -> Done
        let patch = serde_json::json!({ "status": "done" });
        update_requirement_db(&conn, "r1", patch).unwrap();

        let loaded = load_requirements_from_db(&conn).unwrap();
        assert_eq!(loaded[0].status, RequirementStatus::Done);
    }

    #[test]
    fn test_remove_requirement() {
        let _guard = TempDb::new();
        let conn = test_conn();

        add_requirement_db(&conn, make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();
        add_requirement_db(&conn, make_requirement("r2", "/proj/b", RequirementStatus::Todo)).unwrap();

        remove_requirement_db(&conn, "r1").unwrap();

        let loaded = load_requirements_from_db(&conn).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "r2");
    }

    #[test]
    fn test_get_requirements_for_project() {
        let _guard = TempDb::new();
        let conn = test_conn();

        add_requirement_db(&conn, make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();
        add_requirement_db(&conn, make_requirement("r2", "/proj/b", RequirementStatus::Todo)).unwrap();
        add_requirement_db(&conn, make_requirement("r3", "/proj/a", RequirementStatus::Done)).unwrap();

        let proj_a = get_requirements_for_project_db(&conn, "/proj/a").unwrap();
        assert_eq!(proj_a.len(), 2);
    }

    #[test]
    fn test_update_requirement_invalid_status() {
        let _guard = TempDb::new();
        let conn = test_conn();

        add_requirement_db(&conn, make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();

        let patch = serde_json::json!({ "status": "invalid" });
        let result = update_requirement_db(&conn, "r1", patch);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("无效 status"));
    }
}
