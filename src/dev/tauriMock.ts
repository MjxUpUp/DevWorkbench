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

const sessions = [
  { id: 's1', prompt: '重构 providers 设置页交互', agentType: 'claude_code', status: 'completed', projectId: 'p1', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-14T10:00:00.000Z', finishedAt: '2025-06-14T11:00:00.000Z', outputPath: null, exitCode: 0, model: 'sonnet', costUsd: 0.12, tokenUsage: null },
  { id: 's2', prompt: '修复 OpaqueAgent honesty 审计', agentType: 'claude_code', status: 'completed', projectId: 'p1', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-13T14:00:00.000Z', finishedAt: '2025-06-13T15:30:00.000Z', outputPath: null, exitCode: 0, model: 'sonnet', costUsd: 0.08, tokenUsage: null },
  { id: 's3', prompt: 'DAG Human 节点审批闭环', agentType: 'codex', status: 'running', projectId: 'p1', projectPath: 'E:/DevWorkbench', startedAt: '2025-06-15T09:00:00.000Z', finishedAt: null, outputPath: null, exitCode: null, model: null, costUsd: null, tokenUsage: null },
];

const knowledge = (q: string) => [
  { id: 'k1', title: `${q} 相关的崩溃修复经验`, content: '...', category: 'bug', confidence: 0.92, projectPath: 'E:/DevWorkbench', projectHash: null, sourceAgent: 'claude_code', sourceSessionId: 's1', sourceType: 'session', createdAt: '2025-06-14T00:00:00.000Z' },
  { id: 'k2', title: `${q} 的架构决策记录`, content: '...', category: 'decision', confidence: 0.85, projectPath: null, projectHash: null, sourceAgent: null, sourceSessionId: null, sourceType: null, createdAt: '2025-06-13T00:00:00.000Z' },
];

const workflows = [
  { id: 'w1', name: '示例审批流', yaml_content: 'start: prompt_1\nend: gate_1\nnodes:\n  prompt_1:\n    type: prompt\n    text: "demo"\n  gate_1:\n    type: gate\n    gate: forge\nedges:\n  - { from: prompt_1, to: gate_1 }\n', created_at: '2025-06-01T00:00:00.000Z', updated_at: '2025-06-01T00:00:00.000Z' },
];

const providers = {
  providers: [
    { id: 'anthropic', name: 'Anthropic', kind: 'anthropic', baseUrl: 'https://api.anthropic.com', apiKeyEnv: 'ANTHROPIC_API_KEY', models: ['claude-sonnet-4-6', 'claude-opus-4-8'], enabled: true },
    { id: 'glm', name: '智谱 GLM', kind: 'openai', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', apiKeyEnv: 'GLM_API_KEY', models: ['glm-4.6', 'glm-4.5-air'], enabled: true },
  ],
  active: 'anthropic',
};

const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
  load_projects: () => projects,
  get_sessions_for_project: () => sessions,
  load_sessions: () => sessions,
  search_knowledge: (args) => knowledge(String(args.query ?? '')),
  get_knowledge_for_project: () => [],
  list_workflows: () => workflows,
  load_settings: () => ({ theme: 'light', language: 'zh-CN', editor: 'vscode', terminal: 'pwsh' }),
  save_settings: () => null,
  get_providers_config: () => providers,
  set_providers_config: () => null,
  test_provider_connection: () => ({ ok: true, latency_ms: 42 }),
  detect_tools: () => ({ git: { installed: true, version: '2.45.0' }, node: { installed: true, version: '20.10.0' }, rust: { installed: true, version: '1.78.0' } }),
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
    { agentType: 'claude_code', displayName: 'Claude Code', installed: true },
    { agentType: 'codex', displayName: 'Codex CLI', installed: true },
  ],
  recommend_agent_for_project: () => 'claude_code',
  list_skills: () => [],
  skill_catalog: () => [],
  mcp_catalog: () => [],
  mcp_servers: () => [],
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
