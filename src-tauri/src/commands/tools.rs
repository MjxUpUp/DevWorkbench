use crate::models::{AgentType, ToolStatus};

/// macOS GUI 应用 PATH 不包含 brew 等路径，需要手动扩展
#[cfg(target_os = "macos")]
fn expanded_paths() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        format!("{}/.cargo/bin", home),
        format!("{}/.npm-global/bin", home),
        format!("{}/.local/bin", home),
    ]
}

/// 在扩展 PATH 中查找可执行文件
#[cfg(target_os = "macos")]
pub(crate) fn which_expanded(name: &str) -> Option<std::path::PathBuf> {
    // 先尝试默认 which
    if let Ok(p) = which::which(name) {
        return Some(p);
    }
    // 再搜索扩展路径
    for dir in expanded_paths() {
        let candidate = std::path::Path::new(&dir).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn which_expanded(name: &str) -> Option<std::path::PathBuf> {
    which::which(name).ok()
}

/// Non-agent tools that are detected alongside agents (IDE, VCS)
const NON_AGENT_TOOLS: &[&str] = &["code", "git"];

#[tauri::command]
pub fn detect_tools() -> Vec<ToolStatus> {
    // 读取用户自定义路径
    let custom_paths = crate::commands::projects::load_settings()
        .ok()
        .map(|s| s.tool_paths)
        .unwrap_or_default();

    let mut results = Vec::new();

    // Agent tools — derived from AgentType enum (single source of truth)
    for agent_type in AgentType::all() {
        let cmd = agent_type.command_name();
        results.push(detect_one(cmd, &custom_paths));
    }

    // Non-agent tools (IDE, VCS)
    for &name in NON_AGENT_TOOLS {
        results.push(detect_one(name, &custom_paths));
    }

    results
}

fn detect_one(name: &str, custom_paths: &std::collections::HashMap<String, String>) -> ToolStatus {
    // 优先级：1. 用户自定义路径  2. which 查找（含扩展 PATH）
    if let Some(custom) = custom_paths.get(name) {
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
}
