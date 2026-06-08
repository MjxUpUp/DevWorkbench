use crate::models::Session;
use std::fs;
use std::path::PathBuf;

// Thread-local override for agents_dir (test only).
// Each test thread gets its own isolated directory.
#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_AGENTS_DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn agents_dir() -> Result<PathBuf, String> {
    #[cfg(test)]
    {
        let override_path = TEST_AGENTS_DIR_OVERRIDE.with(|cell| cell.borrow().clone());
        if let Some(dir) = override_path {
            if !dir.exists() {
                fs::create_dir_all(&dir).map_err(|e| format!("创建 agents 目录失败: {}", e))?;
            }
            return Ok(dir);
        }
    }
    let home = crate::commands::projects::dirs_home();
    let dir = home.join(".dev-workbench").join("agents");
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("创建 agents 目录失败: {}", e))?;
    }
    Ok(dir)
}

fn sessions_file() -> Result<PathBuf, String> {
    agents_dir().map(|d| d.join("sessions.json"))
}

pub fn load_sessions() -> Result<Vec<Session>, String> {
    let path = sessions_file()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("读取 sessions 失败: {}", e))?;
    if content.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str(&content).map_err(|e| format!("解析 sessions 失败: {}", e))
}

pub fn save_sessions(sessions: &[Session]) -> Result<(), String> {
    let path = sessions_file()?;
    let json = serde_json::to_string_pretty(sessions).map_err(|e| format!("序列化 sessions 失败: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("写入 sessions 失败: {}", e))
}

pub fn add_session(session: Session) -> Result<Vec<Session>, String> {
    let mut sessions = load_sessions()?;
    sessions.push(session);
    save_sessions(&sessions)?;
    Ok(sessions)
}

pub fn update_session(id: &str, patch: serde_json::Value) -> Result<Vec<Session>, String> {
    let mut sessions = load_sessions()?;
    let session = sessions.iter_mut().find(|s| s.id == id)
        .ok_or_else(|| format!("Session {} 不存在", id))?;

    if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
        session.status = match status {
            "running" => crate::models::SessionStatus::Running,
            "completed" => crate::models::SessionStatus::Completed,
            "failed" => crate::models::SessionStatus::Failed,
            _ => return Err(format!("无效 status: {}", status)),
        };
    }
    if let Some(exit_code) = patch.get("exitCode").or_else(|| patch.get("exit_code")).and_then(|v| v.as_i64()) {
        session.exit_code = Some(exit_code as i32);
    }
    if let Some(finished_at) = patch.get("finishedAt").or_else(|| patch.get("finished_at")).and_then(|v| v.as_str()) {
        session.finished_at = Some(finished_at.to_string());
    }
    if let Some(summary) = patch.get("outputSummary").or_else(|| patch.get("output_summary")).and_then(|v| v.as_str()) {
        session.output_summary = Some(summary.to_string());
    }
    if let Some(snap) = patch.get("contextSnapshot").or_else(|| patch.get("context_snapshot")) {
        session.context_snapshot = serde_json::from_value(snap.clone()).ok();
    }

    save_sessions(&sessions)?;
    Ok(sessions)
}

pub fn get_sessions_for_project(project_path: &str) -> Result<Vec<Session>, String> {
    let sessions = load_sessions()?;
    Ok(sessions.into_iter().filter(|s| s.project_path == project_path).collect())
}

pub fn get_running_sessions() -> Result<Vec<Session>, String> {
    let sessions = load_sessions()?;
    Ok(sessions.into_iter().filter(|s| s.status == crate::models::SessionStatus::Running).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentType, SessionStatus};

    /// RAII guard: sets thread-local agents_dir override, clears on drop.
    struct TempAgentsDir {
        _tmp: tempfile::TempDir,
    }

    impl TempAgentsDir {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let dir = tmp.path().to_path_buf();
            TEST_AGENTS_DIR_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(dir));
            Self { _tmp: tmp }
        }
    }

    impl Drop for TempAgentsDir {
        fn drop(&mut self) {
            TEST_AGENTS_DIR_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
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
        let _guard = TempAgentsDir::new();

        add_session(make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();
        add_session(make_session("s2", "/proj/b", SessionStatus::Completed)).unwrap();

        let loaded = load_sessions().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "s1");
        assert_eq!(loaded[1].id, "s2");
    }

    #[test]
    fn test_update_session_status() {
        let _guard = TempAgentsDir::new();

        add_session(make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        let patch = serde_json::json!({ "status": "completed", "exitCode": 0 });
        let result = update_session("s1", patch).unwrap();
        assert_eq!(result[0].status, SessionStatus::Completed);
        assert_eq!(result[0].exit_code, Some(0));
    }

    #[test]
    fn test_get_sessions_for_project() {
        let _guard = TempAgentsDir::new();

        add_session(make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();
        add_session(make_session("s2", "/proj/b", SessionStatus::Completed)).unwrap();
        add_session(make_session("s3", "/proj/a", SessionStatus::Completed)).unwrap();

        let proj_a = get_sessions_for_project("/proj/a").unwrap();
        assert_eq!(proj_a.len(), 2);
        assert!(proj_a.iter().all(|s| s.project_path == "/proj/a"));
    }

    #[test]
    fn test_get_running_sessions() {
        let _guard = TempAgentsDir::new();

        add_session(make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();
        add_session(make_session("s2", "/proj/b", SessionStatus::Completed)).unwrap();
        add_session(make_session("s3", "/proj/c", SessionStatus::Running)).unwrap();

        let running = get_running_sessions().unwrap();
        assert_eq!(running.len(), 2);
    }

    #[test]
    fn test_update_session_invalid_status() {
        let _guard = TempAgentsDir::new();

        add_session(make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        let patch = serde_json::json!({ "status": "invalid_status" });
        let result = update_session("s1", patch);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("无效 status"));
    }
}
