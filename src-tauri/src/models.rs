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
    /// Palette flavor (v3 theme switching), orthogonal to `theme` (light/dark).
    /// Must match the TS union "pi" | "ink" | "moss". serde(default) so a JSON
    /// blob (legacy settings.json import via migrate_v7_to_v8) missing the key
    /// fills it with default_palette()="pi". Persisted in the columnar `settings`
    /// table via load_settings_from_db / save_settings_to_db — those read/write
    /// the column explicitly (added by migrate_v18_to_v19), so this default only
    /// fires on JSON deserialization, not on the DB path.
    #[serde(default = "default_palette")]
    pub palette: String,
    #[serde(default)]
    pub cli_flags: std::collections::HashMap<String, String>,
    /// Whether the user has completed the first-run onboarding wizard.
    /// false on a fresh install → the wizard overlay shows; flipped to true
    /// when the user finishes it (or relaunches it from Settings). serde(default)
    /// so a legacy JSON blob missing the key doesn't block startup. Persisted in
    /// the columnar `settings` table via load_settings_from_db /
    /// save_settings_to_db — those read/write the column explicitly (added by
    /// migrate_v19_to_v20), so this default only fires on JSON deserialization.
    #[serde(default)]
    pub onboarding_completed: bool,
}

fn default_theme() -> String {
    // Must match the TS union "light" | "dark" | "auto".
    "auto".to_string()
}

fn default_palette() -> String {
    // Must match the TS union "pi" | "ink" | "moss". "pi" = pi.dev warm-paper
    // default (per the TS AppSettings.palette doc comment).
    "pi".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// Current HEAD branch name (or "HEAD (detached)"). This is the only field
    /// the frontend reads — TitleBar/StatusBar show it as the breadcrumb branch.
    /// The richer dirty/ahead-behind/line-stat fields were dropped together with
    /// their sole consumer (GitPanel, removed in the workspace-refactor).
    pub branch: String,
}

// ---- Agent Hub types ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    // NOTE: do NOT derive Copy — contains variants without trivial copy
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
            Self::ReactKernel => "Kernel Agent",
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
    /// The user deliberately stopped the session (stop_agent_session). Distinct
    /// from `Failed` so the UI renders "已取消" rather than "失败" — the run was
    /// intentionally halted, not crashed. stop_agent_session writes this; it was
    /// previously rejected by update_session_db's status validator (which only
    /// allowed running/completed/failed), making EVERY user stop return Err.
    Cancelled,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
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
    /// The Forge task this session is bound to (set when spawned under an
    /// active task). Drives TaskGuardHook's boundary check: writes inside the
    /// task's working_dir pass, writes outside are blocked, and a session with
    /// no task only warns (never bricks the agent). None for sessions spawned
    /// without a task, or rows predating the v11→v12 migration (column added
    /// then, defaults NULL).
    #[serde(default)]
    pub task_ref: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigFile {
    pub servers: Vec<McpServerConfig>,
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

// ---- Slash command types ----

/// A user-defined `/`-command: a named prompt template with `$ARGUMENTS` /
/// `$0` / `$1`.. placeholders rendered when the user submits `/name args`.
/// Seeded with the four built-ins (/plan /review /test /fix); the table is the
/// single source of truth the `/` trigger menu reads from (no more hardcoded
/// BUILTIN_COMMANDS in the frontend).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    pub id: String,
    /// Command name WITHOUT the leading slash (e.g. "plan").
    pub name: String,
    pub description: Option<String>,
    /// Prompt template. `$ARGUMENTS`/`$0` = all args; `$1`..`$n` = split tokens.
    pub template: String,
    pub category: Option<String>,
    pub created_at: String,
}

// ---- User-configurable lifecycle hooks (D2) ----

/// Which lifecycle event a [`UserHook`] fires on. Mirrors the `event` column;
/// persisted as the snake_case kebab used in [`HookEvent`] dispatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserHookEvent {
    /// Fires when a new session is starting (first turn). The hook's stdout
    /// (exit 0) is injected into the first turn's prompt as session-level
    /// context. exit 2 is logged but cannot stop the session.
    SessionStart,
    /// Fires when a new user prompt is about to be sent to the model. The
    /// hook's stdout (exit 0) is injected as additional context.
    UserPromptSubmit,
    /// Fires before each tool invocation. Exit 2 BLOCKS that tool call (the
    /// tool does not run; the block reason becomes its result).
    PreToolUse,
    /// Fires after each tool returns. Observation only — exit 2 is logged but
    /// cannot retroactively un-execute the tool.
    PostToolUse,
    /// Fires before context auto-compaction runs. Exit 2 BLOCKS this compaction
    /// round (history left untouched; next turn retries). exit 0 stdout is
    /// logged (v1 — a future revision may pass it as a keep-hint to the
    /// summarizer).
    PreCompact,
    /// Fires when the agent run stops (completed / failed / aborted). Output is
    /// ignored — the hook runs for its side effect (notifications, cleanup).
    Stop,
}

impl UserHookEvent {
    /// Persisted column value (snake_case), matching the DB seed/CRUD contract.
    pub fn as_db(&self) -> &'static str {
        match self {
            UserHookEvent::SessionStart => "session_start",
            UserHookEvent::UserPromptSubmit => "user_prompt_submit",
            UserHookEvent::PreToolUse => "pre_tool_use",
            UserHookEvent::PostToolUse => "post_tool_use",
            UserHookEvent::PreCompact => "pre_compact",
            UserHookEvent::Stop => "stop",
        }
    }

    /// Parse a stored column value back into the enum. Unknown strings error so
    /// a corrupt row surfaces loudly instead of silently skipping.
    pub fn from_db(s: &str) -> Result<Self, String> {
        match s {
            "session_start" => Ok(UserHookEvent::SessionStart),
            "user_prompt_submit" => Ok(UserHookEvent::UserPromptSubmit),
            "pre_tool_use" => Ok(UserHookEvent::PreToolUse),
            "post_tool_use" => Ok(UserHookEvent::PostToolUse),
            "pre_compact" => Ok(UserHookEvent::PreCompact),
            "stop" => Ok(UserHookEvent::Stop),
            other => Err(format!("unknown user_hook event: {other}")),
        }
    }
}

/// A user-defined lifecycle hook (D2). One row = one shell command bound to a
/// single event. Loaded at agent build time and registered into the
/// HookManager; its `on_event` runs the command and (for UserPromptSubmit)
/// returns stdout as injected context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserHook {
    pub id: String,
    pub name: String,
    pub event: UserHookEvent,
    /// Shell command. Run via `sh -c` when `shell` is true (default).
    pub command: String,
    pub shell: bool,
    pub timeout_secs: u64,
    pub enabled: bool,
    /// Optional tool-name matcher (claude-code `matcher`), meaningful only for
    /// PreToolUse / PostToolUse. `None` / empty / `"*"` = match all. Three modes:
    /// literal exact, pipe `|` alternation, regex (see `matches_pattern`).
    /// `#[serde(default)]` so rows / payloads predating the v12→v13 migration
    /// (no column) still deserialize as None.
    #[serde(default)]
    pub matcher: Option<String>,
    pub created_at: String,
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
    /// B5 transparent cost: prompt-cache tokens (Anthropic
    /// cache_read_input_tokens / cache_creation_input_tokens). 0 for providers
    /// that don't report cache usage, or for pre-v17 rows. serde(default)
    /// so older serialized records still deserialize.
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
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
    /// B5 transparent cost: per-tier token totals + the per-tier USD split, so
    /// the dashboard shows "input $X · output $Y · cache $Z" instead of one
    /// opaque number. The split is derived from per-model token sums × the
    /// pricing table (aggregate_costs groups by model). serde(default) so older
    /// clients that don't expect these fields still parse the response.
    #[serde(default)]
    pub total_cache_read_tokens: i64,
    #[serde(default)]
    pub total_cache_write_tokens: i64,
    #[serde(default)]
    pub input_cost: f64,
    #[serde(default)]
    pub output_cost: f64,
    #[serde(default)]
    pub cache_read_cost: f64,
    #[serde(default)]
    pub cache_write_cost: f64,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// B2: a legacy JSON blob (from the v0.7 settings.json migration path or an
    /// older app version that never knew about `palette`) must deserialize with
    /// palette filled by default_palette()="pi", NOT fail and NOT silently
    /// become empty. This is the round-trip the settings table column also
    /// relies on (load_settings_from_db → AppSettings).
    #[test]
    fn app_settings_legacy_json_without_palette_defaults_to_pi() {
        // Mirrors a settings.json written before palette shipped — every other
        // field present, palette absent.
        let json = r#"{
            "scan_directories": ["/tmp"],
            "tool_paths": {"go": "/usr/bin/go"},
            "theme": "dark",
            "cli_flags": {"pi": "--model glm"}
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("legacy JSON must parse");
        assert_eq!(
            s.palette, "pi",
            "missing palette must fall back to default 'pi'"
        );
        assert_eq!(s.theme, "dark");
    }

    /// B2: empty JSON object — every field is serde(default), so all of them
    /// (including palette) take their default. Guards against a regression that
    /// makes palette required and breaks deserialization of `{}`.
    #[test]
    fn app_settings_empty_json_uses_all_defaults_including_palette() {
        let s: AppSettings = serde_json::from_str("{}").expect("{} must parse");
        assert_eq!(s.palette, "pi");
        assert_eq!(s.theme, "auto");
        assert!(s.scan_directories.is_empty());
    }

    /// B2: serialize → deserialize round-trip preserves an explicitly-set
    /// palette. Without the field on the struct, serde would silently drop it
    /// on the save side; this test pins that the column value "ink" survives a
    /// full JSON round-trip (which is what save_settings_to_db effectively does
    /// for the complex sub-fields, and what the v7→v8 settings.json migration
    /// did for the whole struct).
    #[test]
    fn app_settings_roundtrip_preserves_palette() {
        let original = AppSettings {
            scan_directories: vec!["/a".into()],
            tool_paths: std::collections::HashMap::new(),
            theme: "light".into(),
            palette: "moss".into(),
            cli_flags: std::collections::HashMap::new(),
            onboarding_completed: false,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.palette, "moss", "palette must survive round-trip");
        assert_eq!(back.theme, "light");
    }

    /// B2: default_palette() is the single source the DB column DEFAULT, the
    /// serde fallback, and the TS default all agree on. Pinning the value keeps
    /// a future "rename default to 'pi-dev'" change honest.
    #[test]
    fn default_palette_is_pi() {
        assert_eq!(default_palette(), "pi");
    }
}
