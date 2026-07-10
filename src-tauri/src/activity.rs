use crate::error::AppError;
use crate::models::{ActivityEvent, AgentType};
use rusqlite::params;
use sha2::{Digest, Sha256};

/// Hash a project path to a consistent identifier.
pub fn hash_project_path(project_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project_path.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)[..16].to_string()
}

/// Convenience constructor for ActivityEvent.
pub fn make_activity_event(
    session_id: &str,
    project_path: &str,
    agent_type: &crate::models::AgentType,
    event_type: &str,
    title: &str,
    description: Option<String>,
    files_changed: Option<Vec<String>>,
) -> ActivityEvent {
    ActivityEvent {
        id: uuid::Uuid::new_v4().to_string(),
        project_hash: hash_project_path(project_path),
        agent_type: agent_type.clone(),
        event_type: event_type.to_string(),
        title: title.to_string(),
        description,
        files_changed,
        session_id: Some(session_id.to_string()),
        timestamp: chrono::Local::now().to_rfc3339(),
        metadata: None,
    }
}

/// Record an activity event in the database.
pub fn record_event(conn: &rusqlite::Connection, event: &ActivityEvent) -> Result<(), AppError> {
    let files_json = event
        .files_changed
        .as_ref()
        .map(|f| serde_json::to_string(f).unwrap_or_else(|_| "[]".to_string()))
        .unwrap_or_else(|| "null".to_string());
    let metadata_json = event
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_else(|_| "null".to_string()))
        .unwrap_or_else(|| "null".to_string());

    conn.execute(
        "INSERT INTO activity_events
            (id, project_hash, agent_type, event_type, title, description,
             files_changed, session_id, timestamp, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            event.id,
            event.project_hash,
            serde_json::to_string(&event.agent_type)?.trim_matches('"'),
            event.event_type,
            event.title,
            event.description,
            files_json,
            event.session_id,
            event.timestamp,
            metadata_json,
        ],
    )?;
    Ok(())
}

/// Get all activity events for a specific project.
pub fn get_events_for_project(
    conn: &rusqlite::Connection,
    project_path: &str,
) -> Result<Vec<ActivityEvent>, AppError> {
    let hash = hash_project_path(project_path);
    let mut stmt = conn.prepare(
        "SELECT id, project_hash, agent_type, event_type, title, description,
                files_changed, session_id, timestamp, metadata
         FROM activity_events WHERE project_hash = ?1 ORDER BY timestamp DESC",
    )?;

    let events = stmt.query_map(params![hash], |row| {
        let agent_type_str: String = row.get(2)?;
        let agent_type: AgentType =
            serde_json::from_value(serde_json::Value::String(agent_type_str))
                .unwrap_or(AgentType::ReactKernel);

        let files_str: Option<String> = row.get(6)?;
        let files_changed: Option<Vec<String>> = files_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let meta_str: Option<String> = row.get(9)?;
        let metadata: Option<serde_json::Value> = meta_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(ActivityEvent {
            id: row.get(0)?,
            project_hash: row.get(1)?,
            agent_type,
            event_type: row.get(3)?,
            title: row.get(4)?,
            description: row.get(5)?,
            files_changed,
            session_id: row.get(7)?,
            timestamp: row.get(8)?,
            metadata,
        })
    })?;

    let mut result = Vec::new();
    for e in events {
        result.push(e?);
    }
    Ok(result)
}

/// Get recent activity events across all projects.
pub fn get_recent_events(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<ActivityEvent>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_hash, agent_type, event_type, title, description,
                files_changed, session_id, timestamp, metadata
         FROM activity_events ORDER BY timestamp DESC LIMIT ?1",
    )?;

    let events = stmt.query_map(params![limit as i64], |row| {
        let agent_type_str: String = row.get(2)?;
        let agent_type: AgentType =
            serde_json::from_value(serde_json::Value::String(agent_type_str))
                .unwrap_or(AgentType::ReactKernel);

        let files_str: Option<String> = row.get(6)?;
        let files_changed: Option<Vec<String>> = files_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let meta_str: Option<String> = row.get(9)?;
        let metadata: Option<serde_json::Value> = meta_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(ActivityEvent {
            id: row.get(0)?,
            project_hash: row.get(1)?,
            agent_type,
            event_type: row.get(3)?,
            title: row.get(4)?,
            description: row.get(5)?,
            files_changed,
            session_id: row.get(7)?,
            timestamp: row.get(8)?,
            metadata,
        })
    })?;

    let mut result = Vec::new();
    for e in events {
        result.push(e?);
    }
    Ok(result)
}

/// Union of every `files_changed` recorded across a session's activity events
/// — the P3 trajectory preview (FullTrajectory.files_changed) and the P6
/// rubric's `file_change` dimension both need the set of paths a run actually
/// touched. Aggregates both the per-write `FileChanged` rows and the run-end
/// git-diff snapshot (whichever an event carried), de-duplicated + sorted for a
/// stable wire order. Empty for a session with no recorded file events.
pub fn files_changed_for_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT files_changed FROM activity_events
         WHERE session_id = ?1 AND files_changed IS NOT NULL",
    )?;
    let mut seen = std::collections::HashSet::new();
    let rows = stmt.query_map(params![session_id], |row| {
        let s: Option<String> = row.get(0)?;
        Ok(s)
    })?;
    for r in rows {
        if let Some(s) = r? {
            if let Ok(v) = serde_json::from_str::<Vec<String>>(&s) {
                for f in v {
                    seen.insert(f);
                }
            }
        }
    }
    let mut out: Vec<String> = seen.into_iter().collect();
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    struct TempDb {
        _tmp: tempfile::TempDir,
        conn: rusqlite::Connection,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let conn = db::init_db(&db_path).expect("init_db failed");
            Self { _tmp: tmp, conn }
        }
    }

    fn make_event(id: &str, project: &str, event_type: &str, title: &str) -> ActivityEvent {
        ActivityEvent {
            id: id.to_string(),
            project_hash: hash_project_path(project),
            agent_type: AgentType::ReactKernel,
            event_type: event_type.to_string(),
            title: title.to_string(),
            description: None,
            files_changed: None,
            session_id: None,
            timestamp: chrono::Local::now().to_rfc3339(),
            metadata: None,
        }
    }

    #[test]
    fn test_record_and_get_events() {
        let db = TempDb::new();
        let e1 = make_event("e1", "/proj/a", "session_started", "Started session");
        let e2 = make_event("e2", "/proj/a", "session_completed", "Completed session");
        let e3 = make_event("e3", "/proj/b", "session_started", "Started session");

        record_event(&db.conn, &e1).unwrap();
        record_event(&db.conn, &e2).unwrap();
        record_event(&db.conn, &e3).unwrap();

        let proj_a = get_events_for_project(&db.conn, "/proj/a").unwrap();
        assert_eq!(proj_a.len(), 2);

        let proj_b = get_events_for_project(&db.conn, "/proj/b").unwrap();
        assert_eq!(proj_b.len(), 1);

        let recent = get_recent_events(&db.conn, 10).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn files_changed_for_session_unions_across_events() {
        let db = TempDb::new();
        // Two events for s1 carrying different file sets (a per-write row +
        // the run-end aggregated snapshot), plus an unrelated session.
        let mut e1 = make_event("e1", "/proj", "file_changed", "edit a");
        e1.session_id = Some("s1".into());
        e1.files_changed = Some(vec!["src/a.ts".into(), "src/b.ts".into()]);
        let mut e2 = make_event("e2", "/proj", "session_completed", "done");
        e2.session_id = Some("s1".into());
        e2.files_changed = Some(vec!["src/b.ts".into(), "src/c.rs".into()]);
        let mut e3 = make_event("e3", "/proj", "session_completed", "other");
        e3.session_id = Some("s2".into());
        e3.files_changed = Some(vec!["src/x.go".into()]);
        record_event(&db.conn, &e1).unwrap();
        record_event(&db.conn, &e2).unwrap();
        record_event(&db.conn, &e3).unwrap();

        // s1 → union {a,b} ∪ {b,c} = {a,b,c}, de-duplicated + sorted.
        let files = files_changed_for_session(&db.conn, "s1").unwrap();
        assert_eq!(files, vec!["src/a.ts", "src/b.ts", "src/c.rs"]);
        // s2 isolated.
        assert_eq!(files_changed_for_session(&db.conn, "s2").unwrap(), vec!["src/x.go"]);
        // Unknown session → empty (not an error).
        assert!(files_changed_for_session(&db.conn, "nope").unwrap().is_empty());
    }

    #[test]
    fn test_hash_project_path_consistent() {
        let h1 = hash_project_path("/foo/bar");
        let h2 = hash_project_path("/foo/bar");
        let h3 = hash_project_path("/foo/baz");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 16);
    }
}
