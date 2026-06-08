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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommend_rust_project() {
        let tags = vec!["Rust".to_string(), "Tauri".to_string()];
        assert_eq!(recommend_agent(&tags), Some(AgentType::ClaudeCode));
    }

    #[test]
    fn test_recommend_python_project() {
        let tags = vec!["Python".to_string(), "Django".to_string()];
        assert_eq!(recommend_agent(&tags), Some(AgentType::Codex));
    }

    #[test]
    fn test_recommend_frontend_project() {
        let tags = vec!["React".to_string(), "TypeScript".to_string()];
        assert_eq!(recommend_agent(&tags), Some(AgentType::CursorAgent));
    }

    #[test]
    fn test_recommend_vue_project() {
        let tags = vec!["Vue".to_string()];
        assert_eq!(recommend_agent(&tags), Some(AgentType::CursorAgent));
    }

    #[test]
    fn test_recommend_nextjs_project() {
        let tags = vec!["Next.js".to_string()];
        assert_eq!(recommend_agent(&tags), Some(AgentType::CursorAgent));
    }

    #[test]
    fn test_recommend_default_unknown() {
        let tags = vec!["Go".to_string(), "Docker".to_string()];
        // No specific rule → defaults to Claude Code
        assert_eq!(recommend_agent(&tags), Some(AgentType::ClaudeCode));
    }

    #[test]
    fn test_recommend_empty_tags() {
        assert_eq!(recommend_agent(&[]), Some(AgentType::ClaudeCode));
    }

    #[test]
    fn test_recommend_rust_priority_over_frontend() {
        // If both Rust and React tags present, Rust rule wins (checked first)
        let tags = vec!["React".to_string(), "Rust".to_string()];
        assert_eq!(recommend_agent(&tags), Some(AgentType::ClaudeCode));
    }
}
