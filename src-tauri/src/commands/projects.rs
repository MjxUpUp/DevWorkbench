use crate::models::{AppSettings, Project};
use std::fs;
use std::path::PathBuf;

fn data_dir() -> Result<PathBuf, String> {
    let home = dirs_home();
    let dir = home.join(".dev-workbench");
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("创建数据目录失败: {}", e))?;
    }
    Ok(dir)
}

fn dirs_home() -> PathBuf {
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

fn projects_file() -> Result<PathBuf, String> {
    data_dir().map(|d| d.join("projects.json"))
}

fn settings_file() -> Result<PathBuf, String> {
    data_dir().map(|d| d.join("settings.json"))
}

#[tauri::command]
pub fn load_projects() -> Result<Vec<Project>, String> {
    let path = projects_file()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("读取项目文件失败: {}", e))?;
    let projects: Vec<Project> = serde_json::from_str(&content)
        .map_err(|e| format!("解析项目文件失败: {}", e))?;
    Ok(projects)
}

#[tauri::command]
pub fn save_projects(projects: Vec<Project>) -> Result<(), String> {
    let path = projects_file()?;
    let content = serde_json::to_string_pretty(&projects)
        .map_err(|e| format!("序列化项目数据失败: {}", e))?;
    fs::write(&path, content)
        .map_err(|e| format!("写入项目文件失败: {}", e))
}

/// 原子添加项目：load → push → save → 返回完整数组
#[tauri::command]
pub fn add_project(project: Project) -> Result<Vec<Project>, String> {
    let mut projects = load_projects()?;
    projects.push(project);
    save_projects(projects.clone())?;
    Ok(projects)
}

/// 原子删除项目：load → retain → save → 返回完整数组
#[tauri::command]
pub fn remove_project(id: String) -> Result<Vec<Project>, String> {
    let mut projects = load_projects()?;
    let before = projects.len();
    projects.retain(|p| p.id != id);
    if projects.len() == before {
        return Err(format!("项目 {} 不存在", id));
    }
    save_projects(projects.clone())?;
    Ok(projects)
}

/// 原子更新项目：load → patch → save → 返回完整数组
#[tauri::command]
pub fn update_project(id: String, patch: serde_json::Value) -> Result<Vec<Project>, String> {
    let mut projects = load_projects()?;
    let found = projects.iter_mut().find(|p| p.id == id)
        .ok_or_else(|| format!("项目 {} 不存在", id))?;

    // 逐字段 patch，只更新提供的字段
    if let Some(v) = patch.get("name").and_then(|v| v.as_str()) {
        found.name = v.to_string();
    }
    if let Some(v) = patch.get("description").and_then(|v| v.as_str()) {
        found.description = v.to_string();
    }
    if let Some(v) = patch.get("path").and_then(|v| v.as_str()) {
        found.path = v.to_string();
    }
    if let Some(arr) = patch.get("tags").and_then(|v| v.as_array()) {
        found.tags = arr.iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
    }
    if let Some(v) = patch.get("coverImage").or_else(|| patch.get("cover_image")) {
        found.cover_image = v.as_str().map(String::from);
    }
    if let Some(v) = patch.get("starred").and_then(|v| v.as_bool()) {
        found.starred = v;
    }
    if let Some(arr) = patch.get("workspaceTools").or_else(|| patch.get("workspace_tools")).and_then(serde_json::Value::as_array) {
        found.workspace_tools = arr.iter()
            .filter_map(|t: &serde_json::Value| t.as_str().map(String::from))
            .collect();
    }

    save_projects(projects.clone())?;
    Ok(projects)
}

#[tauri::command]
pub fn load_settings() -> Result<AppSettings, String> {
    let path = settings_file()?;
    if !path.exists() {
        return Ok(AppSettings {
            scan_directories: Vec::new(),
            tool_paths: std::collections::HashMap::new(),
        });
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("读取设置文件失败: {}", e))?;
    let settings: AppSettings = serde_json::from_str(&content)
        .map_err(|e| format!("解析设置文件失败: {}", e))?;
    Ok(settings)
}

#[tauri::command]
pub fn save_settings(settings: AppSettings) -> Result<(), String> {
    let path = settings_file()?;
    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("序列化设置失败: {}", e))?;
    fs::write(&path, content)
        .map_err(|e| format!("写入设置失败: {}", e))
}

#[tauri::command]
pub fn update_project_open(id: String) -> Result<Vec<Project>, String> {
    let mut projects = load_projects()?;
    let now = chrono::Local::now().to_rfc3339();
    for p in &mut projects {
        if p.id == id {
            p.open_count += 1;
            p.last_opened_at = Some(now);
            break;
        }
    }
    save_projects(projects.clone())?;
    Ok(projects)
}

#[tauri::command]
pub fn record_tool_open(id: String, tool_name: String) -> Result<Vec<Project>, String> {
    let mut projects = load_projects()?;
    for p in &mut projects {
        if p.id == id {
            // 去重：先移除已有的同名工具
            p.last_opened_tools.retain(|t| t != &tool_name);
            // 插入到最前面（最近使用的在前）
            p.last_opened_tools.insert(0, tool_name);
            // 保留最多 5 个
            p.last_opened_tools.truncate(5);
            break;
        }
    }
    save_projects(projects.clone())?;
    Ok(projects)
}
