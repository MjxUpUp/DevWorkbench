use crate::models::AgentType;

/// Information about a discovered agent
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub agent_type: AgentType,
    pub installed: bool,
    pub path: Option<String>,
    pub supports_resume: bool,
}

/// Auto-discover installed agents. Extends the existing tool detection
/// with agent-specific capability information.
pub fn discover_agents() -> Vec<AgentInfo> {
    let custom_paths = crate::commands::projects::load_settings()
        .ok()
        .map(|s| s.tool_paths)
        .unwrap_or_default();

    AgentType::all()
        .into_iter()
        .map(|agent_type| {
            let cmd = agent_type.command_name();
            let custom = custom_paths.get(cmd).cloned().filter(|p| !p.is_empty());

            let (installed, path) = if let Some(ref custom_path) = custom {
                (true, Some(custom_path.clone()))
            } else {
                match crate::commands::tools::which_expanded(cmd) {
                    Some(p) => (true, Some(p.to_string_lossy().to_string())),
                    None => (false, None),
                }
            };

            let supports_resume =
                matches!(agent_type, AgentType::ClaudeCode | AgentType::Codex);

            AgentInfo {
                agent_type,
                installed,
                path,
                supports_resume,
            }
        })
        .collect()
}

/// Get agent recommendation based on project tech-stack tags
pub fn recommend_agent(tags: &[String]) -> Option<AgentType> {
    if tags.iter().any(|t| t == "Rust" || t == "Tauri") {
        return Some(AgentType::ClaudeCode);
    }
    if tags.iter().any(|t| t == "Python") {
        return Some(AgentType::Codex);
    }
    if tags.iter().any(|t| t == "React" || t == "Vue" || t == "Next.js" || t == "Frontend") {
        return Some(AgentType::CursorAgent);
    }
    Some(AgentType::ClaudeCode)
}
