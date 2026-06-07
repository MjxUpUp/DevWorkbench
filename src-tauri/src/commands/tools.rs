use crate::models::ToolStatus;

#[tauri::command]
pub fn detect_tools() -> Vec<ToolStatus> {
    let tools = ["claude", "cursor", "code", "git", "pi", "codex"];
    tools
        .iter()
        .map(|name| match which::which(name) {
            Ok(path) => ToolStatus {
                name: name.to_string(),
                installed: true,
                path: Some(path.to_string_lossy().to_string()),
            },
            Err(_) => ToolStatus {
                name: name.to_string(),
                installed: false,
                path: None,
            },
        })
        .collect()
}
