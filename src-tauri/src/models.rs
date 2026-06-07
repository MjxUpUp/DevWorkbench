use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: String,
    pub tags: Vec<String>,
    pub cover_image: Option<String>,
    pub open_count: u32,
    pub last_opened_at: Option<String>,
    pub starred: bool,
    pub created_at: String,
    #[serde(default)]
    pub last_opened_tools: Vec<String>,
    #[serde(default)]
    pub workspace_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatus {
    pub name: String,
    pub installed: bool,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub scan_directories: Vec<String>,
    #[serde(default)]
    pub tool_paths: std::collections::HashMap<String, String>,
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "obsidian".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub branch: String,
    pub is_dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub last_commit_time: Option<String>,
}
