export interface Project {
  id: string;
  name: string;
  description: string;
  path: string;
  tags: string[];
  cover_image: string | null;
  open_count: number;
  last_opened_at: string | null;
  starred: boolean;
  created_at: string;
  last_opened_tools: string[];
  workspace_tools: string[];
}

export interface ToolStatus {
  name: string;
  installed: boolean;
  path: string | null;
}

export interface AppSettings {
  scan_directories: string[];
  tool_paths: Record<string, string>;
  /** light | dark | auto (auto follows the OS via prefers-color-scheme) */
  theme: 'light' | 'dark' | 'auto';
  preferred_terminal: string;
  cli_flags: Record<string, string>;
}

export interface TerminalInfo {
  id: string;
  label: string;
  available: boolean;
}

export interface GitRepo {
  path: string;
  name: string;
}

export interface GitStatus {
  branch: string;
  isDirty: boolean;
  ahead: number;
  behind: number;
  lastCommitTime: string | null;
  /** Lines added (tracked HEAD→worktree + untracked file contents). */
  insertions: number;
  /** Lines deleted (tracked HEAD→worktree). */
  deletions: number;
}

// ---- Agent Hub types ----

export type AgentType =
  | 'claude_code'
  | 'codex'
  | 'cursor_agent'
  | 'gemini_cli'
  | 'copilot'
  | 'qwen_code'
  | 'pi'
  | 'react_kernel';

export type SessionStatus = 'running' | 'completed' | 'failed';

export interface AgentInfo {
  agentType: AgentType;
  displayName: string;
  commandName: string;
  installed: boolean;
  path: string | null;
  supportsResume: boolean;
}

export interface FileDiff {
  path: string;
  added: number;
  removed: number;
}

export interface ContextSnapshot {
  filesChanged: string[];
  keyOutput: string;
  /** Per-file line stats from `git diff --numstat`. Optional because older
   *  sessions persisted before this field existed only have filesChanged. */
  fileDiffs?: FileDiff[];
}

// ---- Shadow-git checkpoint (v1.2 T6) ----

/** Working-tree snapshot captured at ReactAgent session start, enabling one-
 *  click rollback of the agent's changes. camelCase matches the Rust struct's
 *  `#[serde(rename_all = "camelCase")]`. */
export interface Checkpoint {
  sessionId: string;
  projectPath: string;
  createdAt: string;
  headSha: string;
  stashSha: string | null;
  untrackedAtCheckpoint: string[];
  reason: string;
}

/** Outcome of rolling a session back to its checkpoint. */
export interface RollbackResult {
  restoredFiles: string[];
  removedUntracked: string[];
  skipped: string[];
}

/** 一个 conversation 内单个 turn 的分支树节点(扁平,带 parent 指针)。
 *  前端按 parentId 分组渲染分支切换器:同一 parent 下的多个节点互为兄弟分支,
 *  edit_and_regenerate fork 出的新 turn 就是某条 turn 的兄弟。 */
export interface BranchNode {
  id: string;
  parentId: string | null;
  prompt: string;
  status: string;
  startedAt: string;
  agentType: string;
}

export interface Session {
  id: string;
  projectPath: string;
  agentType: AgentType;
  status: SessionStatus;
  prompt: string;
  model: string | null;
  startedAt: string;
  finishedAt: string | null;
  exitCode: number | null;
  outputSummary: string | null;
  contextSnapshot: ContextSnapshot | null;
  linkedRequirementId: string | null;
  parentSessionId: string | null;
  /** The multi-turn conversation this turn (session) belongs to. A conversation
   *  is the Claude-Code-style "topic" container; a session is now one turn of
   *  it. Backfilled for pre-v1.1 data by migrate_v9_to_v10. */
  conversationId: string | null;
  /** Persisted chat blocks (text/tool_use/tool_result) written at finalize so a
   *  historical session replays via BlocksView instead of the raw terminal log.
   *  null/undefined for raw agents (no agent:event stream) or pre-G1 sessions. */
  blocks?: ChatStreamEvent[] | null;
  /** Forge task this session is bound to (drives the kernel TaskGuard scope
   *  check: writes inside the task's working_dir pass, outside are blocked;
   *  a taskless session only warns, never blocks). null for unbound sessions. */
  taskRef?: string | null;
  tokenUsage?: number;
  estimatedCost?: number;
}

/** A `/`-command prompt template. `name` has no leading slash. The kernel
 *  expands `/name args` at submit time: `$ARGUMENTS`/`$0` = all args, `$1`..`$n`
 *  = split tokens. Seeded with /plan /review /test /fix. */
export interface SlashCommand {
  id: string;
  name: string;
  description: string | null;
  template: string;
  category: string | null;
  createdAt: string;
}

/**
 * Lifecycle event a user hook fires on (D2). Mirrors the Rust `UserHookEvent`
 * serde schema: serialized as snake_case over the wire.
 *   - 'user_prompt_submit' — stdout (exit 0) injected as context before the turn
 *   - 'pre_tool_use'    — before each tool call; exit 2 BLOCKS that tool
 *   - 'post_tool_use'   — after each tool returns; observation only (exit 2 logged)
 *   - 'stop' — runs for side effects at run end (output ignored)
 */
export type UserHookEvent = 'user_prompt_submit' | 'pre_tool_use' | 'post_tool_use' | 'stop';

/**
 * A user-configurable lifecycle hook (D2). One row = one shell command bound to
 * a single event. Mirrors the Rust `UserHook` serde schema (camelCase fields).
 */
export interface UserHook {
  id: string;
  name: string;
  event: UserHookEvent;
  /** Shell command. Run via `sh -c` (Unix) / `cmd /C` (Windows) when `shell` is true. */
  command: string;
  shell: boolean;
  timeoutSecs: number;
  enabled: boolean;
  /**
   * Optional tool-name matcher (claude-code `matcher`), meaningful only for
   * pre_tool_use / post_tool_use. null/empty = match all. Three modes: exact
   * (`write_file`), pipe alternation (`write_file|edit`), regex (`^write_`).
   */
  matcher: string | null;
  createdAt: string;
}

/**
 * Scope where a sub-agent lives. Mirrors the Rust `scope` param of
 * save_subagent / delete_subagent.
 *   - 'global'  — ~/.agents/subagents (shared across projects)
 *   - 'project' — <project>/.agents/subagents (versioned with the repo)
 */
export type SubAgentScope = 'global' | 'project';

/**
 * A named sub-agent (D1) surfaced to the UI. One row = one
 * `.agents/subagents/<name>/AGENT.md`. The kernel loads these at agent build
 * time, so the main agent can delegate by name via dispatch_subagent.
 * Mirrors the Rust `SubAgentInfo` serde schema (camelCase fields).
 */
export interface SubAgentInfo {
  name: string;
  description: string;
  systemPrompt: string;
  /** Tool-name prefixes the child is restricted to (empty = full read-only set). */
  toolsAllow: string[];
  scope: string;
  /** Absolute path of the AGENT.md on disk (for display). */
  sourcePath: string;
}

/**
 * A conversation = the multi-turn topic container (one Claude-Code "session").
 * A `Session` is now one turn inside it. Conversations live under a project;
 * turns inside a conversation may switch agents (claude → codex → …).
 */
export interface Conversation {
  id: string;
  projectPath: string;
  title: string;
  /** The agent of the most recent turn. Null only if the conversation has no
   *  turns yet (shouldn't happen in practice — creating one always spawns turn 1). */
  lastAgent: AgentType | null;
  status: string;
  startedAt: string;
  lastActivityAt: string;
  pinned: boolean;
}

// ---- Activity types ----

export interface ActivityEvent {
  id: string;
  projectHash: string;
  agentType: AgentType;
  eventType: string;
  title: string;
  description: string | null;
  filesChanged: string[] | null;
  sessionId: string | null;
  timestamp: string;
  metadata: unknown;
}

// ---- Knowledge types ----

export interface KnowledgeEntry {
  id: string;
  projectHash: string;
  category: string;
  title: string;
  content: string;
  sourceAgent: AgentType;
  sourceSessionId: string | null;
  sourceType: string;
  confidence: number;
  createdAt: string;
  updatedAt: string;
  accessCount: number;
}

// ---- Skill types (mirror Rust models::Skill, serde camelCase) ----

export interface Skill {
  id: string;
  org: string;
  name: string;
  version?: string | null;
  installedAt?: string | null;
  path?: string | null;
  qualityScore?: number | null;
  metadata?: string | null;
  description?: string | null;
  icon?: string | null;
  category?: string | null;
  securityScore?: number | null;
  installs?: number | null;
  rating?: number | null;
  author?: string | null;
  compatibleAgents?: string | null;
  qualityDetails?: string | null;
  securityDetails?: string | null;
  configSchema?: string | null;
}

/** One discoverable skill on disk (Rust skills_cmds::SkillCatalogEntry). */
export interface SkillCatalogEntry {
  name: string;
  description: string;
  source: string;
  scope: 'global' | 'project';
}

// ---- Quality types ----

export interface QualityCheck {
  name: string;
  status: 'passed' | 'failed' | 'warning' | 'skipped';
  message: string | null;
}

export interface QualityReport {
  id: string;
  sessionId: string;
  checks: QualityCheck[];
  overallStatus: string;
  createdAt: string;
}

// ---- Config types ----

export interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
  targetAgents: AgentType[];
}

export interface McpConfigFile {
  servers: McpServerConfig[];
}

/** One tool advertised by one connected MCP server (Rust mcp_cmds::
 * McpToolListing, the `mcp_catalog` "what can I use right now" row). */
export interface McpToolListing {
  server: string;
  name: string;
  description: string;
  inputSchema: unknown;
}

// ---- Provider types ----

export interface ModelEntry {
  id: string;
  label: string;
  enabled: boolean;
  /** Model context window in tokens. Drives auto-compaction threshold
   * (75% of window). Omitted → backend falls back to a conservative 32k
   * default. Mirrors Rust `ModelEntry::context_window` (serde camelCase). */
  contextWindow?: number;
}

export interface ProviderConfig {
  id: string;
  name: string;
  endpoint: string;
  apiKey: string;
  enabled: boolean;
  models: ModelEntry[];
}

export interface ProvidersConfig {
  providers: ProviderConfig[];
  modelMapping: Record<string, string>;
}

// ---- File listing types ----

export interface FileEntry {
  path: string;
  name: string;
  isDir: boolean;
}

// ---- Dashboard types ----

export interface CostSummary {
  totalCost: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  sessionCount: number;
}

export interface CostTrendPoint {
  date: string;
  cost: number;
  tokens: number;
}

export interface BudgetSettings {
  monthlyBudgetUsd: number | null;
  alertThreshold: number;
}

export interface DashboardStats {
  todayCost: number;
  costTrend: number;
  totalTokens: number;
  tokenTrend: number;
  activeSessions: number;
  qualityRate: number;
}

export interface BudgetInfo {
  spent: number;
  total: number;
  percentage: number;
}

export interface QualityEntry {
  sessionId: string;
  sessionNumber: number;
  score: number;
  total: number;
  agent: string;
  tokens: number;
  status: 'pass' | 'warn' | 'fail';
}

// ---- Workflow types ----

export interface Workflow {
  id: string;
  name: string;
  yamlContent: string;
  createdAt: string;
  updatedAt: string;
}

/** A built-in workflow template (`list_workflow_templates`) — a starter YAML the
 * user clones into the YAML editor instead of authoring the DAG from scratch.
 * Mirrors Rust `commands::workflows::WorkflowTemplate` (serde camelCase). */
export interface WorkflowTemplate {
  name: string;
  description: string;
  category: string;
  yamlContent: string;
}

// Note: WorkflowRun / WorkflowStep removed — the static run-tracking model was
// never written to. Execution is now stream-based via the kernel-compose Graph
// engine: run_workflow returns a { run_id, output } result and emits live
// `workflow:progress` events the Orchestrate canvas subscribes to.

/** Result of `invoke('run_workflow', { yamlContent, input, workingDir })`. */
export interface WorkflowRunResult {
  run_id: string;
  output: unknown;
}

/** GraphEvent kinds emitted as `workflow:progress` payload.runId === run_id. */
export type WorkflowProgressEvent =
  | { kind: 'node_start'; node: string }
  | { kind: 'node_end'; node: string; status: 'pending' | 'running' | 'done' | 'failed' | 'skipped' | 'waiting_approval'; error?: string }
  | { kind: 'approval_required'; node: string; prompt: string; resume_token: string }
  | { kind: 'node_output'; node: string; chunk: unknown }
  | { kind: 'graph_done'; output: unknown }
  | { kind: 'graph_failed'; error: string };

/** The full `workflow:progress` Tauri event payload. */
export interface WorkflowProgressPayload {
  runId: string;
  event: WorkflowProgressEvent;
}

// ---- Chat block stream types ----

/** Wire-level structured event from the `agent:event` channel — one per parsed
 *  block of an agent's output (claude stream-json today; ReactAgent later).
 *  The chat UI folds these into block cards (text / tool call / tool result /
 *  result). Mirrors the Rust `ChatStreamEvent` serde schema exactly: tag is
 *  "kind", field names are verbatim (snake_case, NO camelCase). */
export type ChatStreamEvent =
  | { kind: 'text'; content: string }
  | { kind: 'thinking'; content: string }
  | { kind: 'tool_use'; name: string; input: unknown }
  | { kind: 'tool_result'; content: string; is_error: boolean }
  | { kind: 'result'; is_error: boolean; secs: number }
  | { kind: 'file_changed'; path: string };

// ---- LLM trace observability types ----

/** One persisted LLM HTTP call (Rust `trace::db::LlmTraceRow`). Mirrors the
 *  Rust serde schema EXACTLY — verbatim snake_case (NO camelCase), because
 *  LlmTraceRow has no #[serde(rename_all)]. One row per GlmChatModel
 *  stream/generate request. This is the observability layer that finally makes
 *  a 0.8s "GLM stream failed: 400" session diagnosable: the real request body
 *  and the provider's error response body are both persisted here. */
export interface LlmTrace {
  id: string;
  session_id: string | null;
  conversation_id: string | null;
  model: string;
  base_url: string;
  /** HTTP status code. null when the call never reached HTTP (network error,
   *  decode failure before a response). */
  status_code: number | null;
  /** non_2xx | network | decode | stream | circuit. null on a clean 2xx. */
  error_kind: string | null;
  /** The request body (build_body JSON), truncated to ~32KB on the Rust side.
   *  Safe to persist — api_key travels in a header, never the body. */
  req_body: string;
  /** The raw wire response body, truncated to ~32KB on the Rust side. On a clean
   *  2xx this is the full response (JSON for generate, the SSE stream for
   *  stream) so the request↔response pair is one query away — symmetric with the
   *  error path, which stores the provider's error JSON. null only when the call
   *  never produced a body (network error / decode failure before a response). */
  resp_body: string | null;
  latency_ms: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  created_at: string;
}

/** Trace retention settings — mirrors the Rust `trace::db::TraceSettings` row
 *  (verbatim snake_case, no serde rename). `retention_days` null = infinite
 *  (the default, per the 2026-06-19 trace observability research — Phoenix's
 *  infinite-by-default semantics); a positive N prunes traces older than N days
 *  on startup. `last_vacuum_at` throttles VACUUM to weekly and is shown in the
 *  settings UI. */
export interface TraceSettings {
  retention_days: number | null;
  last_vacuum_at: string | null;
}
