use crate::models::ToolStatus;

/// macOS GUI 应用 PATH 不包含 brew 等路径，需要手动扩展
#[cfg(target_os = "macos")]
fn expanded_paths() -> Vec<&'static str> {
    vec![
        "/opt/homebrew/bin",
        "/usr/local/bin",
        &format!("{}/.cargo/bin", std::env::var("HOME").unwrap_or_default()),
        &format!("{}/.npm-global/bin", std::env::var("HOME").unwrap_or_default()),
        &format!("{}/.local/bin", std::env::var("HOME").unwrap_or_default()),
    ]
}

/// 在扩展 PATH 中查找可执行文件
#[cfg(target_os = "macos")]
fn which_expanded(name: &str) -> Option<std::path::PathBuf> {
    // 先尝试默认 which
    if let Ok(p) = which::which(name) {
        return Some(p);
    }
    // 再搜索扩展路径
    for dir in expanded_paths() {
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn which_expanded(name: &str) -> Option<std::path::PathBuf> {
    which::which(name).ok()
}

#[tauri::command]
pub fn detect_tools() -> Vec<ToolStatus> {
    let tools = ["claude", "cursor", "code", "git", "pi", "codex"];

    // 读取用户自定义路径
    let custom_paths = crate::commands::projects::load_settings()
        .ok()
        .map(|s| s.tool_paths)
        .unwrap_or_default();

    tools
        .iter()
        .map(|name| {
            // 优先级：1. 用户自定义路径  2. which 查找（含扩展 PATH）
            if let Some(custom) = custom_paths.get(*name) {
                if !custom.is_empty() {
                    return ToolStatus {
                        name: name.to_string(),
                        installed: true,
                        path: Some(custom.clone()),
                    };
                }
            }

            match which_expanded(name) {
                Some(path) => ToolStatus {
                    name: name.to_string(),
                    installed: true,
                    path: Some(path.to_string_lossy().to_string()),
                },
                None => ToolStatus {
                    name: name.to_string(),
                    installed: false,
                    path: None,
                },
            }
        })
        .collect()
}
