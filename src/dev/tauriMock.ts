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
 * sessions, settings, providers, tools, git). It does
 * NOT emulate stateful/native commands (spawn_agent_session)
 * — those need the real backend (axum HTTP bridge or
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

// Conversations = the topic containers (Claude-Code "session" semantics). Both
// under p1 so the sidebar list shows them when p1 is active. lastAgent is always
// react_kernel now (sole agent after retiring the external CLI link).
const conversations = [
  { id: 'c1', projectPath: 'E:/DevWorkbench', title: '重构 providers 设置页交互', lastAgent: 'react_kernel', status: 'active', startedAt: '2025-06-14T10:00:00.000Z', lastActivityAt: '2025-06-15T09:00:00.000Z', pinned: false },
  { id: 'c2', projectPath: 'E:/DevWorkbench', title: '修复 OpaqueAgent honesty 审计', lastAgent: 'react_kernel', status: 'active', startedAt: '2025-06-13T14:00:00.000Z', lastActivityAt: '2025-06-13T15:30:00.000Z', pinned: true },
];

// Sessions = turns. All turns use react_kernel (sole agent). The running turn
// (s3) belongs to c1. All Session required fields populated so the TS contract
// (and AgentMessage rendering) doesn't read undefined.
const sessions = [
  { id: 's1', prompt: '重构 providers 设置页交互', agentType: 'react_kernel', status: 'completed', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-14T10:00:00.000Z', finishedAt: '2025-06-14T11:00:00.000Z', model: 'sonnet', exitCode: 0, outputSummary: '完成了 providers 设置页交互重构：拆分为三栏布局…', contextSnapshot: null, linkedRequirementId: null, parentSessionId: null, conversationId: 'c1' },
  { id: 's2', prompt: '修复 OpaqueAgent honesty 审计', agentType: 'react_kernel', status: 'completed', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-13T14:00:00.000Z', finishedAt: '2025-06-13T15:30:00.000Z', model: 'sonnet', exitCode: 0, outputSummary: '修复了 OpaqueAgent honesty 审计的断言弱化检测…', contextSnapshot: null, linkedRequirementId: null, parentSessionId: null, conversationId: 'c2' },
  { id: 's3', prompt: '复核并补测试', agentType: 'react_kernel', status: 'running', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-15T09:00:00.000Z', finishedAt: null, model: null, exitCode: null, outputSummary: null, contextSnapshot: null, linkedRequirementId: null, parentSessionId: 's1', conversationId: 'c1' },
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
  load_sessions: () => sessions,
  list_conversations: (args) => {
    const projectPath = String(args.projectPath ?? '');
    return conversations.filter((c) => c.projectPath === projectPath);
  },
  update_conversation: () => null,
  load_settings: () => ({ scan_directories: [], tool_paths: {}, theme: 'light', palette: 'pi', onboarding_completed: false }),
  save_settings: () => null,
  get_providers_config: () => providers,
  set_providers_config: () => null,
  test_provider_connection: () => ({ ok: true, status: 200, message: '连接成功 (mock)' }),
  // ToolStatus[] contract — {name, installed, path} per item. detect_tools now
  // returns only the non-agent tools (code/git); the agent command_name loop
  // was removed with the CLI agent retirement.
  detect_tools: () => [
    { name: 'code', installed: true, path: '/usr/local/bin/code' },
    { name: 'git', installed: true, path: '/usr/bin/git' },
  ],
  get_git_status: () => ({ branch: 'feature/kernel-refactor' }),
  get_recent_activity: () => [],
  get_project_activity: () => [],
  get_cost_summary: () => ({ total_usd: 1.23, by_agent: {}, by_day: [] }),
  get_cost_trend: () => [],
  load_budget: () => ({ monthly_usd: 50 }),
  load_mcp_config: () => ({ servers: [] }),
  list_skills: () => [],
  skill_catalog: () => [],
  mcp_catalog: () => [],
  mcp_servers: () => [],
  // D5: MCP fine-grained CRUD / live-reconnect.
  // The mcp_* stubs are best-effort no-ops (stateful — the real backend owns them).
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
