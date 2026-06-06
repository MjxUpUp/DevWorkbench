use crate::models::{AppSettings, Project};
use std::fs;
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    let home = dirs_home();
    let dir = home.join(".dev-workbench");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
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

fn projects_file() -> PathBuf {
    data_dir().join("projects.json")
}

fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

#[tauri::command]
pub fn load_projects() -> Result<Vec<Project>, String> {
    let path = projects_file();
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
    let path = projects_file();
    let content = serde_json::to_string_pretty(&projects)
        .map_err(|e| format!("序列化项目数据失败: {}", e))?;
    fs::write(&path, content)
        .map_err(|e| format!("写入项目文件失败: {}", e))
}

#[tauri::command]
pub fn load_settings() -> Result<AppSettings, String> {
    let path = settings_file();
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
    let path = settings_file();
    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("序列化设置失败: {}", e))?;
    fs::write(&path, content)
        .map_err(|e| format!("写入设置文件失败: {}", e))
}

#[tauri::command]
pub fn update_project_open(id: String, projects: Vec<Project>) -> Result<Vec<Project>, String> {
    let mut updated = projects;
    let now = chrono::Local::now().to_rfc3339();
    for p in &mut updated {
        if p.id == id {
            p.open_count += 1;
            p.last_opened_at = Some(now);
            break;
        }
    }
    save_projects(updated.clone())?;
    Ok(updated)
}
