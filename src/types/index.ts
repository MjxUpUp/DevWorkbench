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
  /** 调色板风格：pi (pi.dev 暖纸，默认) | ink (墨砚) | moss (苔藓)。
 *  v3 三套主题切换——与 theme (亮/暗) 正交，组合出 6 种外观。 */
  palette: 'pi' | 'ink' | 'moss';
  /** Whether the user finished the first-run onboarding wizard. false on a fresh
 *  install → the wizard overlay shows; flipped true on completion. */
  onboarding_completed: boolean;
}

export interface GitRepo {
  path: string;
  name: string;
}

export interface GitStatus {
  /** Current HEAD branch name. Only field the frontend reads (breadcrumb). */
  branch: string;
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

export type SessionStatus = 'running' | 'completed' | 'failed' | 'cancelled';

/** Agent execution mode. 'executing' is Mission Phase 2 (D4) — set internally by
 *  `mission_apply`, not surfaced for manual selection. UI 模式选择器已完全移除（用户
 *  决定），保留类型供 agentStore 签名 + 后端 PermissionMode wire 对齐：前端不再传
 *  mode，后端用默认；破坏性操作由 ApprovalModal 在触发时承接。 */
export type AgentMode =
  | 'default'
  | 'auto-edit'
  | 'plan'
  | 'executing'
  | 'dry-run'
  | 'silent'
  | 'skip-permissions'
  | 'human-gate';

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
export type UserHookEvent = 'session_start' | 'user_prompt_submit' | 'pre_tool_use' | 'post_tool_use' | 'pre_compact' | 'stop';

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
  /** Free-form JSON (Rust `Option<serde_json::Value>` — serializes as `null`
   *  when absent, NOT omitted). `unknown` forces callers to narrow before
   *  reading; the `| null` reflects that the backend may send JSON null. */
  metadata: unknown | null;
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

/** Wire protocol a provider's endpoint speaks — selects which ChatModel impl
 *  the backend builds (Anthropic Messages vs OpenAI Chat Completions). Mirrors
 *  Rust `ProtocolKind` (serde lowercase). Defaults to 'anthropic'. */
export type ProtocolKind = 'anthropic' | 'openai';

/** A model's routing tier within its provider. `strong` = the capable/expensive
 *  model for hard reasoning; `cheap` = the fast one for trivial steps (tool
 *  results, short confirmations). A provider declaring BOTH gets the per-step
 *  strong/cheap router. Mirrors Rust `ModelTier` (serde lowercase). */
export type ModelTier = 'strong' | 'cheap';

export interface ModelEntry {
  id: string;
  label: string;
  enabled: boolean;
  /** Model context window in tokens. Drives auto-compaction threshold
   * (75% of window). Omitted → backend falls back to a conservative 32k
   * default. Mirrors Rust `ModelEntry::context_window` (serde camelCase). */
  contextWindow?: number;
  /** Routing tier. Omitted/null = this model doesn't participate in per-step
   *  routing. A provider needs one `strong` AND one `cheap` to activate the
   *  router (data-driven version of the old `starts_with("glm-")` guard).
   *  Mirrors Rust `ModelEntry::tier` (serde camelCase). */
  tier?: ModelTier | null;
}

export interface ProviderConfig {
  id: string;
  name: string;
  endpoint: string;
  apiKey: string;
  enabled: boolean;
  /** Wire protocol of this endpoint. Drives the backend's ChatModel impl
   *  selection. Mirrors Rust `ProviderConfig::protocol` (serde camelCase,
   *  defaults to 'anthropic'). */
  protocol?: ProtocolKind;
  models: ModelEntry[];
}

export interface ProvidersConfig {
  providers: ProviderConfig[];
  modelMapping: Record<string, string>;
  /** Schema version (Rust `version: u32` with #[serde(default)]). Optional on read
   *  (older persisted configs omit it); preserved on write by spreading the
   *  loaded object rather than reconstructing it field-by-field. */
  version?: number;
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
  /** B5 transparent cost: per-tier token totals + the per-tier USD split, so the
   *  dashboard shows "input $X · output $Y · cache $Z" instead of one opaque
   *  number. The split is derived backend-side (per-model tokens × pricing).
   *  Optional because older backends / responses may omit them. */
  totalCacheReadTokens?: number;
  totalCacheWriteTokens?: number;
  inputCost?: number;
  outputCost?: number;
  cacheReadCost?: number;
  cacheWriteCost?: number;
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

// ---- Chat block stream types ----

/** Wire-level structured event from the `agent:event` channel — one per parsed
 *  block of an agent's output (claude stream-json today; ReactAgent later).
 *  The chat UI folds these into block cards (text / tool call / tool result /
 *  result). Mirrors the Rust `ChatStreamEvent` serde schema exactly: tag is
 *  "kind", field names are verbatim (snake_case, NO camelCase). */
export type ChatStreamEvent =
  | { kind: 'text'; content: string }
  | { kind: 'thinking'; content: string }
  | {
      kind: 'tool_use';
      name: string;
      input: unknown;
      /** tool_call_id pairing key. Present on the OpaqueAgent path (claude wire
       *  carries `id`); absent on the ReactKernel forward path. Optional so
       *  pre-id session blocks (no `id` field) still parse. */
      id?: string | null;
    }
  | {
      kind: 'tool_result';
      /** Points back to the tool_use block this result answers. Present on the
       *  OpaqueAgent path; absent on the ReactKernel forward path / legacy
       *  blocks. Optional for backward compatibility. */
      tool_use_id?: string | null;
      content: string;
      is_error: boolean;
    }
  | { kind: 'result'; is_error: boolean; secs: number }
  | { kind: 'file_changed'; path: string }
  | {
      kind: 'compact';
      /** One-line human summary of what was archived. The LLM summarize path
       *  carries the model's summary text (anti-injection fence already
       *  stripped on the Rust side); the micro_clear path carries a generic
       *  "已压缩 N 条陈旧工具输出" line. Never the raw fence wrapper. */
      summary: string;
      /** Absolute path to the dropped-messages archive JSONL, or null when
       *  archiving was unavailable (no session id / write failed). The frontend
       *  resolves this against the session id via read_compact_archive_cmd. */
      archived_at: string | null;
      /** Number of messages dropped from model history by this compaction. */
      dropped_count: number;
      /** true on circuit-breaker trip (MAX_CONSECUTIVE_COMPACT_FAILURES) —
       *  renders an error card instead of an info card. */
      is_error: boolean;
    }
  | {
      /** §4.2 缺项3 / CCB `SystemCompactBoundaryMessage` parity: a META event
       *  emitted alongside `compact` when compaction structurally changed the
       *  model's history (summarize / hard-truncate). Never rendered as a chat
       *  block — it's persisted into session.blocks so a resumed session's
       *  blocks_to_history reconstructs a boundary Message, letting
       *  maybe_compact summarize only what came AFTER the last boundary (avoiding
       *  the resume×compact "summary of summary" fidelity drift). MicroClear /
       *  BreakerTripped emit no boundary (no structural change). */
      kind: 'compact_boundary';
      /** "auto" | "manual" — what triggered the compaction. */
      trigger: string;
      /** Estimated tokens just before compaction ran. */
      pre_tokens: number;
      /** Trailing messages preserved verbatim across this compaction. */
      preserved_count: number;
    }
  | {
      kind: 'approval_required';
      /** Tool name about to run (e.g. write_file / bash). */
      tool: string;
      /** Raw JSON arguments string — shown as a destructive-op preview. */
      arguments: string;
      /** Token the resolve command carries back (`approve__{sid}__{seq}`). */
      resume_token: string;
      /** One-line WHY this is destructive (modal title). */
      summary: string;
    };

// ---- LLM trace observability types ----

/** One persisted LLM HTTP call (Rust `trace::db::LlmTraceRow`). Mirrors the
 *  Rust serde schema EXACTLY — verbatim snake_case (NO camelCase), because
 *  LlmTraceRow has no #[serde(rename_all)]. One row per ChatModel
 *  stream/generate request (Anthropic or OpenAI protocol). This is the
 *  observability layer that finally makes a 0.8s "LLM stream failed: 400"
 *  session diagnosable: the real request body and the provider's error
 *  response body are both persisted here. */
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
  /** B3: request-send → first response signal (time-to-first-byte), in ms.
   *  null when the call never reached a first byte (pure network failure) or for
   *  pre-v18 rows. Drives the "model slow to start" diagnosis. */
  ttfb_ms: number | null;
  /** B3: first-byte → completion (output/stream duration), in ms. null when
   *  there was no streaming phase (e.g. headers-only non_2xx) or pre-v18. */
  stream_ms: number | null;
  /** A1 (OTel span tree): the span this call belongs to — one per agent
   *  instance, so all calls one agent makes share its span_id. null for
   *  pre-v22 rows and ad-hoc/test agents (honest absence, not a faked root). */
  span_id: string | null;
  /** A1: the orchestrating agent's span_id (the span that spawned this one).
   *  null for the root agent (top of the tree). */
  parent_span_id: string | null;
  /** A1: human label for the span ("agent" | "subagent" | …) so the tree
   *  renders a name per node instead of a bare id. */
  span_name: string | null;
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

// ---- Subagent board types (C2/D3) ----

/** Terminal status of a dispatched sub-agent. Mirrors the Rust `SubagentStatus`
 *  serde schema (snake_case) — both sides agree on the same 5 values + the
 *  shared parse rules (deer-flow subagent_status_contract.json). */
export type SubagentStatus =
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'timed_out'
  | 'polling_timed_out';

/** One dispatch_subagent call tracked by the subagent board. `status` is
 *  'running' until the matching tool_result resolves it. The optional cost
 *  fields are C2 per-dispatch attribution: the backend appends a
 *  `📊 子 agent 用量: A→B tok · $C` footer to the dispatch's tool_result when the
 *  child model could fork a counting cost sink (production GlmChatModel); absent
 *  on running dispatches, test models, or when the child made no tracked calls. */
export interface SubagentDispatch {
  task: string;
  status: 'running' | SubagentStatus;
  /** C2: total input tokens the dispatched child consumed (sum across its LLM
   *  calls). undefined until the tool_result lands and carries a cost footer. */
  inputTokens?: number;
  /** C2: total output tokens the dispatched child consumed. */
  outputTokens?: number;
  /** C2: total derived USD cost of the dispatched child's LLM calls. */
  costUsd?: number;
}
