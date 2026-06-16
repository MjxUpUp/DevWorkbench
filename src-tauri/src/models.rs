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
    // Must match the TS union "light" | "dark" | "auto".
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub branch: String,
    pub is_dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub last_commit_time: Option<String>,
    /// Lines added across HEAD→worktree (tracked files). Untracked files count
    /// their full content as insertions. 0 when there is no HEAD (empty repo).
    #[serde(default)]
    pub insertions: u64,
    /// Lines deleted across HEAD→worktree (tracked files). 0 when there is no HEAD.
    #[serde(default)]
    pub deletions: u64,
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
    /// Self-hosted ReactAgent (kernel layer). NOT a CLI — never discovered, never
    /// spawned as a subprocess. spawn_agent_session routes kernel=true to the
    /// react_chat driver. Exists as a variant so the DB/session/activity layer
    /// can record + render it like any other agent. Deliberately absent from
    /// all()/from_spec so discovery + workflow dispatch stay CLI-only.
    ReactKernel,
}

impl AgentType {
    pub fn command_name(&self) -> &str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::CursorAgent => "cursor-agent",
            Self::GeminiCli => "gemini",
            Self::Copilot => "github-copilot-cli",
            Self::QwenCode => "qwen",
            Self::Pi => "pi",
            // No CLI binary — ReactKernel runs in-process via the react_chat
            // driver, never as a subprocess. Empty so resolve_agent_exe fails
            // fast if a ReactKernel is ever misrouted to the pty spawn path.
            Self::ReactKernel => "",
        }
    }

    /// Parse an AgentType from a workflow node spec string. Accepts the snake_case
    /// variant name (e.g. "claude_code"), the command name ("claude"), or
    /// display-ish forms. Returns None for unknown agents (the caller may treat
    /// that as a transparent/self-built agent).
    pub fn from_spec(spec: &str) -> Option<Self> {
        match spec.to_ascii_lowercase().as_str() {
            "claude_code" | "claude-code" | "claudecode" | "claude" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "cursor_agent" | "cursor-agent" | "cursor" => Some(Self::CursorAgent),
            "gemini_cli" | "gemini-cli" | "gemini" => Some(Self::GeminiCli),
            "copilot" | "github_copilot_cli" => Some(Self::Copilot),
            "qwen_code" | "qwen-code" | "qwen" => Some(Self::QwenCode),
            "pi" => Some(Self::Pi),
            _ => None,
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
            Self::ReactKernel => "Kernel Agent (GLM)",
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
    /// Which conversation this turn belongs to. None on sessions created before
    /// the v9→v10 migration (backfilled by migrate_v9_to_v10).
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Persisted chat blocks (text/tool_use/tool_result) as a JSON array,
    /// written at finalize so a historical session replays via BlocksView
    /// instead of falling back to the raw terminal log. None for raw agents
    /// (no agent:event stream → no blocks) or aborted sessions. The runtime
    /// in-memory `sessionBlocks` map is the live source during a run; this DB
    /// column is the source of truth once the session is finalized.
    #[serde(default)]
    pub blocks: Option<serde_json::Value>,
}

/// A conversation — a multi-turn dialogue container, the equivalent of a Claude
/// Code session. Holds N [`Session`]s (turns), each potentially by a different
/// agent. Replaces the old flat per-project session list so a coherent dialogue
/// (and agent hand-offs within it) is one resumable unit, not N loose bubbles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub project_path: String,
    pub title: String,
    pub last_agent: Option<AgentType>,
    /// "active" | "archived"
    pub status: String,
    pub started_at: String,
    pub last_activity_at: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub added: i64,
    pub removed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    pub files_changed: Vec<String>,
    pub key_output: String,
    /// Per-file line add/remove counts from `git diff --numstat`. Drives the
    /// "+N / -M" badges in the agent message File Changes block. `#[serde(default)]`
    /// so older context_snapshot JSON (persisted before this field) still
    /// deserializes as an empty list.
    #[serde(default)]
    pub file_diffs: Vec<FileDiff>,
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

// Note: WorkflowRun / WorkflowStep are intentionally removed.
// The previous run-tracking model was never written to — run_workflow was a stub.
// Execution state will be reintroduced in Phase 1 via the kernel-compose Graph engine,
// modeled as AgentEvent streams + checkpoint persistence (not these static rows).

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
