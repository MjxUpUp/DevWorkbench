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
pub enum AgentType { // NOTE: do NOT derive Copy — contains variants without trivial copy
    ClaudeCode,
    Codex,
    CursorAgent,
    GeminiCli,
    Copilot,
    QwenCode,
    Pi,
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
            Self::Pi => "pi",
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
            Self::Pi => "Pi",
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
            Self::Pi,
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

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
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

impl RequirementStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }
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

// ---- Activity types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub id: String,
    pub project_hash: String,
    pub agent_type: AgentType,
    pub event_type: String,
    pub title: String,
    pub description: Option<String>,
    pub files_changed: Option<Vec<String>>,
    pub session_id: Option<String>,
    pub timestamp: String,
    pub metadata: Option<serde_json::Value>,
}

// ---- Knowledge types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEntry {
    pub id: String,
    pub project_hash: String,
    pub category: String,
    pub title: String,
    pub content: String,
    pub source_agent: AgentType,
    pub source_session_id: Option<String>,
    pub source_type: String,
    pub confidence: f64,
    pub created_at: String,
    pub updated_at: String,
    pub access_count: i64,
}

// ---- Quality types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub id: String,
    pub session_id: String,
    pub checks: Vec<QualityCheck>,
    pub overall_status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityCheck {
    pub name: String,
    pub status: String,
    pub message: Option<String>,
}

// ---- Config types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub target_agents: Vec<AgentType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigFile {
    pub servers: Vec<McpServerConfig>,
}

// ---- Workflow types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub yaml_content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub node_type: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub output: Option<String>,
}

// ---- Skill types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub org: String,
    pub name: String,
    pub version: Option<String>,
    pub installed_at: Option<String>,
    pub path: Option<String>,
    pub quality_score: Option<f64>,
    pub metadata: Option<String>,
    // Catalog fields (populated from metadata JSON)
    pub description: Option<String>,
    pub icon: Option<String>,
    pub category: Option<String>,
    pub security_score: Option<f64>,
    pub installs: Option<i64>,
    pub rating: Option<f64>,
    pub author: Option<String>,
    pub compatible_agents: Option<String>,
    pub quality_details: Option<String>,
    pub security_details: Option<String>,
    pub config_schema: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillReport {
    pub id: String,
    pub skill_id: String,
    pub scan_result: String,
    pub scanned_at: String,
}

// ---- Cost types ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRecord {
    pub id: String,
    pub session_id: Option<String>,
    pub agent_type: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSummary {
    pub total_cost: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub session_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTrendPoint {
    pub date: String,
    pub cost: f64,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetSettings {
    pub monthly_budget_usd: Option<f64>,
    pub alert_threshold: f64,
}
