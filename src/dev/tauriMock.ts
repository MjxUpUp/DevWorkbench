/**
 * Dev-only Tauri IPC mock — enables `vite dev` (plain browser) to render the
 * full UI with demo data so Playwright can drive the frontend end-to-end
 * without the Tauri runtime.
 *
 * Why this exists: `@tauri-apps/api`'s `invoke` delegates to
 * `window.__TAURI_INTERNALS__.invoke`, which the Rust core only injects inside
 * the Tauri webview. In a plain browser every `invoke` throws, so stores can't
 * load anything and the UI is blank. This shim injects a fake `invoke` (plus
 * the callback/event stubs `core.js` touches) returning canned demo data.
 *
 * Scope: covers the **data-query** commands the UI renders against (projects,
 * sessions, knowledge, settings, providers, workflows, tools, git). It does
 * NOT emulate stateful/native commands (spawn_agent_session, pty_write_cmd,
 * run_workflow) — those need the real backend (axum HTTP bridge or
 * tauri-driver); see the search-regression E2E notes.
 *
 * Guarded by `import.meta.env.DEV` so production builds tree-shake it out, and
 * by `isTauri()` so it never overrides the real runtime inside the webview.
 */
import { isTauri } from '../utils/env';

const projects = [
  { id: 'p1', name: 'Dev Workbench', description: '主项目', path: 'E:/DevWorkbench', tags: ['tauri', 'react'], cover_image: null, open_count: 42, last_opened_at: null, starred: true, created_at: '2025-01-01T00:00:00.000Z', last_opened_tools: [], workspace_tools: [] },
  { id: 'p2', name: 'Kernel Refactor', description: '', path: 'E:/KernelRefactor', tags: ['rust'], cover_image: null, open_count: 7, last_opened_at: null, starred: false, created_at: '2025-03-01T00:00:00.000Z', last_opened_tools: [], workspace_tools: [] },
  { id: 'p3', name: 'ZCode Adapter', description: '', path: 'E:/ZCodeAdapter', tags: [], cover_image: null, open_count: 3, last_opened_at: null, starred: false, created_at: '2025-05-01T00:00:00.000Z', last_opened_tools: [], workspace_tools: [] },
];

// Conversations = the topic containers (Claude-Code "session" semantics). c1 is
// a cross-agent thread (claude → codex) so the P3 agent-switch divider renders;
// c2 is a single claude turn. Both under p1 so the sidebar list shows them when
// p1 is active.
const conversations = [
  { id: 'c1', projectPath: 'E:/DevWorkbench', title: '重构 providers 设置页交互', lastAgent: 'codex', status: 'active', startedAt: '2025-06-14T10:00:00.000Z', lastActivityAt: '2025-06-15T09:00:00.000Z', pinned: false },
  { id: 'c2', projectPath: 'E:/DevWorkbench', title: '修复 OpaqueAgent honesty 审计', lastAgent: 'claude_code', status: 'active', startedAt: '2025-06-13T14:00:00.000Z', lastActivityAt: '2025-06-13T15:30:00.000Z', pinned: true },
];

// Sessions = turns. c1 has two turns by DIFFERENT agents (the divider case); the
// running turn (s3) belongs to c1 too. All Session required fields populated so
// the TS contract (and AgentMessage rendering) doesn't read undefined.
const sessions = [
  { id: 's1', prompt: '重构 providers 设置页交互', agentType: 'claude_code', status: 'completed', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-14T10:00:00.000Z', finishedAt: '2025-06-14T11:00:00.000Z', model: 'sonnet', exitCode: 0, outputSummary: '完成了 providers 设置页交互重构：拆分为三栏布局…', contextSnapshot: null, linkedRequirementId: null, parentSessionId: null, conversationId: 'c1' },
  { id: 's2', prompt: '修复 OpaqueAgent honesty 审计', agentType: 'claude_code', status: 'completed', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-13T14:00:00.000Z', finishedAt: '2025-06-13T15:30:00.000Z', model: 'sonnet', exitCode: 0, outputSummary: '修复了 OpaqueAgent honesty 审计的断言弱化检测…', contextSnapshot: null, linkedRequirementId: null, parentSessionId: null, conversationId: 'c2' },
  { id: 's3', prompt: '改用 codex 复核并补测试', agentType: 'codex', status: 'running', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-15T09:00:00.000Z', finishedAt: null, model: null, exitCode: null, outputSummary: null, contextSnapshot: null, linkedRequirementId: null, parentSessionId: 's1', conversationId: 'c1' },
];

const knowledge = (q: string) => [
  { id: 'k1', title: `${q} 相关的崩溃修复经验`, content: '...', category: 'bug', confidence: 0.92, projectPath: 'E:/DevWorkbench', projectHash: null, sourceAgent: 'claude_code', sourceSessionId: 's1', sourceType: 'session', createdAt: '2025-06-14T00:00:00.000Z' },
  { id: 'k2', title: `${q} 的架构决策记录`, content: '...', category: 'decision', confidence: 0.85, projectPath: null, projectHash: null, sourceAgent: null, sourceSessionId: null, sourceType: null, createdAt: '2025-06-13T00:00:00.000Z' },
];

const workflows = [
  { id: 'w1', name: '示例审批流', yaml_content: 'start: prompt_1\nend: gate_1\nnodes:\n  prompt_1:\n    type: prompt\n    text: "demo"\n  gate_1:\n    type: gate\n    gate: forge\nedges:\n  - { from: prompt_1, to: gate_1 }\n', created_at: '2025-06-01T00:00:00.000Z', updated_at: '2025-06-01T00:00:00.000Z' },
];

// Wire format mirroring the Rust ProvidersConfig serde schema — the old mock
// used a stale kind/baseUrl/apiKeyEnv/active shape that no consumer matches, so
// the providers store (typed ProvidersConfig) silently rendered defaults. This
// matches what get_providers_config actually returns: ProviderConfig[] with
// endpoint/apiKey/protocol/enabled/models(object[] with tier) + modelMapping.
const providers = {
  providers: [
    {
      id: 'anthropic',
      name: 'Anthropic',
      endpoint: 'https://api.anthropic.com',
      apiKey: 'sk-ant-demo',
      protocol: 'anthropic',
      enabled: true,
      models: [
        { id: 'claude-opus-4-8', label: 'Claude Opus 4.8', enabled: true, tier: 'strong' },
        { id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6', enabled: true, tier: 'cheap' },
      ],
    },
    {
      id: 'zai',
      name: 'Z.AI',
      endpoint: 'https://open.bigmodel.cn/api/anthropic',
      apiKey: 'sk-zai-demo',
      protocol: 'anthropic',
      enabled: true,
      models: [
        { id: 'glm-4.6', label: 'GLM-4.6', enabled: true, tier: 'strong' },
        { id: 'glm-4-flash', label: 'GLM-4-Flash', enabled: true, tier: 'cheap' },
      ],
    },
  ],
  modelMapping: {},
};

export const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
  load_projects: () => projects,
  get_sessions_for_project: () => sessions,
  load_sessions: () => sessions,
  list_conversations: (args) => {
    const projectPath = String(args.projectPath ?? '');
    return conversations.filter((c) => c.projectPath === projectPath);
  },
  update_conversation: () => null,
  read_session_output_cmd: () => null,
  search_knowledge: (args) => knowledge(String(args.query ?? '')),
  get_knowledge_for_project: () => [],
  list_workflows: () => workflows,
  load_settings: () => ({ scan_directories: [], tool_paths: {}, theme: 'light', palette: 'pi', preferred_terminal: '', cli_flags: {}, onboarding_completed: false }),
  save_settings: () => null,
  get_providers_config: () => providers,
  set_providers_config: () => null,
  test_provider_connection: () => ({ ok: true, status: 200, message: '连接成功 (mock)' }),
  // ToolStatus[] contract — {name, installed, path} per item. The old object
  // shape ({git,node,rust}) matched no consumer (useTools/AgentSection both
  // invoke<ToolStatus[]>), so the dev tool-status list silently rendered empty
  // (Array.isArray guard swallowed it). Mirrors Rust detect_tools: all agent
  // command_names + NON_AGENT_TOOLS (code, git).
  detect_tools: () => [
    { name: 'claude', installed: true, path: '/usr/local/bin/claude' },
    { name: 'codex', installed: true, path: '/usr/local/bin/codex' },
    { name: 'code', installed: true, path: '/usr/local/bin/code' },
    { name: 'git', installed: true, path: '/usr/bin/git' },
  ],
  get_git_status: () => ({ branch: 'feature/kernel-refactor', ahead: 0, behind: 0, modified: ['src/App.tsx'], staged: [], untracked: [] }),
  batch_get_git_status: () => projects.map((p) => ({ path: p.path, branch: 'main', ahead: 0, behind: 0, modified: [], staged: [], untracked: [] })),
  get_recent_activity: () => [],
  get_project_activity: () => [],
  get_cost_summary: () => ({ total_usd: 1.23, by_agent: {}, by_day: [] }),
  get_cost_trend: () => [],
  load_budget: () => ({ monthly_usd: 50 }),
  check_budget_alert: () => null,
  load_mcp_config: () => ({ servers: [] }),
  discover_agents_cmd: () => [
    // Full AgentInfo contract (commandName/path/supportsResume) — missing
    // commandName made AgentSection's `key={a.commandName}` collide (every
    // entry key=undefined → React "unique key" warnings). Matches the Rust
    // AgentInfo serde(rename_all = "camelCase") shape.
    { agentType: 'claude_code', displayName: 'Claude Code', commandName: 'claude', installed: true, path: '/usr/local/bin/claude', supportsResume: true },
    { agentType: 'codex', displayName: 'Codex CLI', commandName: 'codex', installed: true, path: '/usr/local/bin/codex', supportsResume: true },
  ],
  recommend_agent_for_project: () => 'claude_code',
  list_skills: () => [],
  skill_catalog: () => [],
  mcp_catalog: () => [],
  mcp_servers: () => [],
  // D5: workflow template starters + MCP fine-grained CRUD / live-reconnect.
  // list_workflow_templates returns demo templates so the OrchestrateView chips
  // render in plain-browser dev mode; the mcp_* stubs are best-effort no-ops
  // (stateful — the real backend owns them).
  list_workflow_templates: () => [
    { name: 'code-review', description: '代码审查 agent + Forge 门禁', category: '质量', yamlContent: 'start: p\nend: g\nnodes:\n  p:\n    type: prompt\n    text: "审查改动"\n  g:\n    type: gate\n    gate: forge\nedges:\n  - { from: p, to: g }\n' },
    { name: 'refactor', description: '重构 agent + 门禁验收', category: '研发', yamlContent: 'start: p\nend: g\nnodes:\n  p:\n    type: prompt\n    text: "重构模块"\n  a:\n    type: agent\n    agent: claude_code\n  g:\n    type: gate\n    gate: forge\nedges:\n  - { from: p, to: a }\n  - { from: a, to: g }\n' },
  ],
  mcp_load_enabled: () => 0,
  mcp_set_enabled: () => null,
  mcp_update_server: () => null,
  mcp_delete_server: () => null,
  scan_git_repos: () => [],
  detect_project_tags: () => [],
};

export function installDevMock(): void {
  if (!import.meta.env.DEV) return;
  if (isTauri()) return; // never override the real runtime

  const w = window as unknown as Record<string, unknown>;
  const internals = (w.__TAURI_INTERNALS__ ?? {}) as Record<string, unknown>;
  w.__TAURI_INTERNALS__ = internals;

  // getCurrentWindow()/getCurrentWebview() read metadata.currentWindow /
  // currentWebview — without these, TitleBar (and anything touching the window
  // API) crashes on mount in a plain browser.
  internals.metadata = {
    currentWindow: { label: 'main' },
    currentWebview: { label: 'main', windowLabel: 'main' },
  };

  internals.invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
    const handler = handlers[cmd];
    if (handler) return handler(args);
    console.warn(`[dev-mock] unhandled invoke: ${cmd}`, args);
    return null;
  };

  // Minimal Channel/callback stubs — core.js reads these for event listen().
  // Returning a sentinel id is enough; no real callbacks fire in mock mode.
  internals.transformCallback = () => 0;
  internals.unregisterCallback = () => undefined;

  const eventInternals = (w.__TAURI_EVENT_PLUGIN_INTERNALS__ ?? {}) as Record<string, unknown>;
  eventInternals.unregisterListener = () => undefined;
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = eventInternals;

  console.info('[dev-mock] Tauri IPC mock installed (DEV, plain-browser mode)');
}
