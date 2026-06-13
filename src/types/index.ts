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
  theme: string;
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
}

// ---- Agent Hub types ----

export type AgentType =
  | 'claude_code'
  | 'codex'
  | 'cursor_agent'
  | 'gemini_cli'
  | 'copilot'
  | 'qwen_code'
  | 'pi';

export type SessionStatus = 'running' | 'completed' | 'failed';

export type RequirementStatus = 'todo' | 'in_progress' | 'done';

export interface AgentInfo {
  agentType: AgentType;
  displayName: string;
  commandName: string;
  installed: boolean;
  path: string | null;
  supportsResume: boolean;
}

export interface ContextSnapshot {
  filesChanged: string[];
  keyOutput: string;
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
  tokenUsage?: number;
  estimatedCost?: number;
}

export interface Requirement {
  id: string;
  projectPath: string;
  title: string;
  description: string | null;
  status: RequirementStatus;
  priority: string | null;
  linkedSessionId: string | null;
  artifacts: string[];
  createdAt: string;
  updatedAt: string;
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

// ---- Provider types ----

export interface ModelEntry {
  id: string;
  label: string;
  enabled: boolean;
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
