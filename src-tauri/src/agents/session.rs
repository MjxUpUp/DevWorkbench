use crate::models::Session;
use std::fs;
use std::path::PathBuf;

pub(crate) fn agents_dir() -> Result<PathBuf, String> {
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
