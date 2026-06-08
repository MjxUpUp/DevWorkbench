use std::path::Path;

/// Trait for all coding agent executors.
/// Each agent (Claude Code, Codex, etc.) implements this trait.
pub trait AgentExecutor: Send + Sync {
    /// The agent type identifier
    fn agent_type(&self) -> crate::models::AgentType;

    /// Build the command to spawn this agent
    fn build_command(
        &self,
        project_path: &Path,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<std::process::Command, String>;

    /// Build the command to resume a session (if supported)
    fn build_resume_command(
        &self,
        _project_path: &Path,
        _session_id: &str,
        _prompt: &str,
    ) -> Result<std::process::Command, String> {
        // Default: not supported, fall back to fresh spawn
        Err(format!(
            "{} 不支持会话恢复",
            self.agent_type().display_name()
        ))
    }

    /// Check if this agent is installed on the system
    fn is_installed(&self) -> bool;

    /// Get the install path (if found)
    fn install_path(&self) -> Option<String>;

    /// Whether this agent supports session resume
    fn supports_resume(&self) -> bool {
        false
    }
}
