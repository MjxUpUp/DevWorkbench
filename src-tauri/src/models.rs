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
    #[serde(default)]
    pub preferred_terminal: String,
    #[serde(default)]
    pub cli_flags: std::collections::HashMap<String, String>,
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

// ---- Agent Hub types ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    ClaudeCode,
    Codex,
    CursorAgent,
    GeminiCli,
    Copilot,
    QwenCode,
}

impl AgentType {
    pub fn command_name(&self) -> &str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::CursorAgent => "cursor-agent",
            Self::GeminiCli => "gemini",
            Self::Copilot => "github-copilot-cli",
            Self::QwenCode => "qwen-code",
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::CursorAgent => "Cursor Agent",
            Self::GeminiCli => "Gemini CLI",
            Self::Copilot => "GitHub Copilot",
            Self::QwenCode => "Qwen Code",
        }
    }

    pub fn all() -> Vec<AgentType> {
        vec![
            Self::ClaudeCode,
            Self::Codex,
            Self::CursorAgent,
            Self::GeminiCli,
            Self::Copilot,
            Self::QwenCode,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub project_path: String,
    pub agent_type: AgentType,
    pub status: SessionStatus,
    pub prompt: String,
    pub model: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub output_summary: Option<String>,
    pub context_snapshot: Option<ContextSnapshot>,
    pub linked_requirement_id: Option<String>,
    pub parent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    pub files_changed: Vec<String>,
    pub key_output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Todo,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    pub id: String,
    pub project_path: String,
    pub title: String,
    pub description: Option<String>,
    pub status: RequirementStatus,
    pub priority: Option<String>,
    pub linked_session_id: Option<String>,
    pub artifacts: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}
