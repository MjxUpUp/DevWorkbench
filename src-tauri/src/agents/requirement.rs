use crate::models::Requirement;
use std::fs;
use std::path::PathBuf;

fn requirements_file() -> Result<PathBuf, String> {
    super::session::agents_dir().map(|d| d.join("requirements.json"))
}

pub fn load_requirements() -> Result<Vec<Requirement>, String> {
    let path = requirements_file()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("读取 requirements 失败: {}", e))?;
    if content.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str(&content).map_err(|e| format!("解析 requirements 失败: {}", e))
}

pub fn save_requirements(requirements: &[Requirement]) -> Result<(), String> {
    let path = requirements_file()?;
    let json = serde_json::to_string_pretty(requirements).map_err(|e| format!("序列化 requirements 失败: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("写入 requirements 失败: {}", e))
}

pub fn add_requirement(mut req: Requirement) -> Result<Vec<Requirement>, String> {
    let mut reqs = load_requirements()?;
    req.updated_at = chrono::Local::now().to_rfc3339();
    reqs.push(req);
    save_requirements(&reqs)?;
    Ok(reqs)
}

pub fn update_requirement(id: &str, patch: serde_json::Value) -> Result<Vec<Requirement>, String> {
    let mut reqs = load_requirements()?;
    let req = reqs.iter_mut().find(|r| r.id == id)
        .ok_or_else(|| format!("Requirement {} 不存在", id))?;

    if let Some(title) = patch.get("title").and_then(|v| v.as_str()) {
        req.title = title.to_string();
    }
    if let Some(desc) = patch.get("description") {
        req.description = desc.as_str().map(|s| s.to_string());
    }
    if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
        req.status = match status {
            "todo" => crate::models::RequirementStatus::Todo,
            "in_progress" => crate::models::RequirementStatus::InProgress,
            "done" => crate::models::RequirementStatus::Done,
            _ => return Err(format!("无效 status: {}", status)),
        };
    }
    if let Some(priority) = patch.get("priority") {
        req.priority = priority.as_str().map(|s| s.to_string());
    }
    if let Some(session_id) = patch.get("linked_session_id") {
        req.linked_session_id = session_id.as_str().map(|s| s.to_string());
    }
    if let Some(artifacts) = patch.get("artifacts") {
        req.artifacts = serde_json::from_value(artifacts.clone()).unwrap_or_default();
    }

    req.updated_at = chrono::Local::now().to_rfc3339();
    save_requirements(&reqs)?;
    Ok(reqs)
}

pub fn remove_requirement(id: &str) -> Result<Vec<Requirement>, String> {
    let mut reqs = load_requirements()?;
    reqs.retain(|r| r.id != id);
    save_requirements(&reqs)?;
    Ok(reqs)
}

pub fn get_requirements_for_project(project_path: &str) -> Result<Vec<Requirement>, String> {
    let reqs = load_requirements()?;
    Ok(reqs.into_iter().filter(|r| r.project_path == project_path).collect())
}
