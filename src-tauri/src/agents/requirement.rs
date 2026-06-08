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
    if let Some(session_id) = patch.get("linkedSessionId") {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RequirementStatus;

    /// RAII guard: sets thread-local agents_dir override, clears on drop.
    struct TempAgentsDir {
        _tmp: tempfile::TempDir,
    }

    impl TempAgentsDir {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let dir = tmp.path().to_path_buf();
            super::super::session::TEST_AGENTS_DIR_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(dir));
            Self { _tmp: tmp }
        }
    }

    impl Drop for TempAgentsDir {
        fn drop(&mut self) {
            super::super::session::TEST_AGENTS_DIR_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
        }
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
        let _guard = TempAgentsDir::new();

        add_requirement(make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();
        add_requirement(make_requirement("r2", "/proj/b", RequirementStatus::InProgress)).unwrap();

        let loaded = load_requirements().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn test_update_requirement_status_transitions() {
        let _guard = TempAgentsDir::new();

        add_requirement(make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();

        // Todo -> InProgress
        let patch = serde_json::json!({ "status": "in_progress" });
        let result = update_requirement("r1", patch).unwrap();
        assert_eq!(result[0].status, RequirementStatus::InProgress);

        // InProgress -> Done
        let patch = serde_json::json!({ "status": "done" });
        let result = update_requirement("r1", patch).unwrap();
        assert_eq!(result[0].status, RequirementStatus::Done);
    }

    #[test]
    fn test_remove_requirement() {
        let _guard = TempAgentsDir::new();

        add_requirement(make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();
        add_requirement(make_requirement("r2", "/proj/b", RequirementStatus::Todo)).unwrap();

        let result = remove_requirement("r1").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "r2");
    }

    #[test]
    fn test_get_requirements_for_project() {
        let _guard = TempAgentsDir::new();

        add_requirement(make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();
        add_requirement(make_requirement("r2", "/proj/b", RequirementStatus::Todo)).unwrap();
        add_requirement(make_requirement("r3", "/proj/a", RequirementStatus::Done)).unwrap();

        let proj_a = get_requirements_for_project("/proj/a").unwrap();
        assert_eq!(proj_a.len(), 2);
    }

    #[test]
    fn test_update_requirement_invalid_status() {
        let _guard = TempAgentsDir::new();

        add_requirement(make_requirement("r1", "/proj/a", RequirementStatus::Todo)).unwrap();

        let patch = serde_json::json!({ "status": "invalid" });
        let result = update_requirement("r1", patch);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("无效 status"));
    }
}
