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
 *   - 'stop' — runs for side effects at run end (output ignored)
 */
export type UserHookEvent = 'user_prompt_submit' | 'stop';

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
  createdAt: string;
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
