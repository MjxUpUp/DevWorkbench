import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { PlanBar } from '../PlanBar';
import { GateBar } from '../GateBar';
import { MemoryRail } from '../MemoryRail';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useAgentStore } from '../../../stores/agentStore';
import { useOrchestrateStore } from '../../../stores/orchestrateStore';
import { useDashboardStore } from '../../../stores/dashboardStore';
import { useKnowledgeStore } from '../../../stores/knowledgeStore';
import type { Session, SessionStatus, CostSummary } from '../../../types';

// GateBar useEffect 调 fetchDashboard → invoke；mock 成 reject 让 fetchDashboard
// catch（不覆盖下方 setState 的 budget/costSummary），隔离测渲染派生。
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.reject(new Error('test: no backend'))),
}));

/**
 * 起底重构 块1-4 契约测试。
 *  4 区结构 + Stage 路由 + Chat 步骤分组的正确性由 E2E 覆盖（7/7）。此处单测纯派生：
 *  - PlanBar：activeView → 模式（轴线A 位置）+ sessions/nodes → plan 进度（执行）
 *  - GateBar：sessions → 运行计数 + budget → 成本门控/熔断（块4）
 */

const mkSession = (status: SessionStatus, over: Partial<Session> = {}): Session => ({
  id: 's-1',
  projectPath: '/p',
  agentType: 'react_kernel',
  status,
  prompt: '',
  model: null,
  startedAt: '2026-01-01T00:00:00Z',
  finishedAt: null,
  exitCode: null,
  outputSummary: null,
  contextSnapshot: null,
  linkedRequirementId: null,
  parentSessionId: null,
  conversationId: null,
  ...over,
});

const toolUse = (name: string) => ({ kind: 'tool_use' as const, name, input: {} });
const mkCost = (totalCost: number) => ({ totalCost }) as unknown as CostSummary;

describe('PlanBar — 视图→运行模式派生（轴线A·位置可见）', () => {
  beforeEach(() => {
    useNavigationStore.setState({
      activeView: 'task',
      activeProject: null,
      selectedConversationId: null,
    });
    useAgentStore.setState({ sessions: [] });
    useOrchestrateStore.setState({ nodes: {} });
  });

  it('task 视图 → Chat Agent，plan∈LLM context', () => {
    useNavigationStore.setState({ activeView: 'task' });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-mode')).toHaveTextContent('Chat Agent');
    expect(screen.getByTestId('plan-bar')).toHaveTextContent(/plan ∈ LLM context/);
  });

  it('orchestrate 视图 → DAG Script，plan∈脚本变量', () => {
    useNavigationStore.setState({ activeView: 'orchestrate' });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-mode')).toHaveTextContent('DAG Script');
    expect(screen.getByTestId('plan-bar')).toHaveTextContent(/plan ∈ 脚本变量/);
  });

  it('trace 视图 → 观测模式', () => {
    useNavigationStore.setState({ activeView: 'trace' });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-mode')).toHaveTextContent('观测');
  });
});

describe('PlanBar — plan 进度派生（轴线A·执行可见，块2）', () => {
  beforeEach(() => {
    useNavigationStore.setState({
      activeView: 'task',
      activeProject: null,
      selectedConversationId: null,
    });
    useAgentStore.setState({ sessions: [] });
    useOrchestrateStore.setState({ nodes: {} });
  });

  it('Chat 模式：running session 的 tool 步骤数 + 状态', () => {
    useNavigationStore.setState({ activeView: 'task', selectedConversationId: 'c1' });
    useAgentStore.setState({
      sessions: [
        mkSession('running', {
          id: 's-run',
          conversationId: 'c1',
          blocks: [toolUse('read_file'), toolUse('write_file'), { kind: 'text', content: 'hi' }],
        }),
      ],
    });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-progress')).toHaveTextContent('步骤 2 · 运行中');
  });

  it('Chat 模式：无选中会话 → 无活跃会话（而非渲染崩溃）', () => {
    useNavigationStore.setState({ activeView: 'task', selectedConversationId: 'c-x' });
    useAgentStore.setState({ sessions: [mkSession('completed', { conversationId: 'c-other' })] });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-progress')).toHaveTextContent('无活跃会话');
  });

  it('DAG 模式：nodes done/total + running 标注', () => {
    useNavigationStore.setState({ activeView: 'orchestrate' });
    useOrchestrateStore.setState({
      nodes: {
        n1: { status: 'done' },
        n2: { status: 'done' },
        n3: { status: 'running' },
      },
    });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-progress')).toHaveTextContent('节点 2/3 · 运行中');
  });

  it('DAG 模式：未加载工作流', () => {
    useNavigationStore.setState({ activeView: 'orchestrate' });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-progress')).toHaveTextContent('未加载工作流');
  });
});

describe('GateBar — 运行态派生（门控层）', () => {
  beforeEach(() => {
    useAgentStore.setState({ sessions: [] });
    useDashboardStore.setState({
      budget: { spent: 0, total: 0, percentage: 0 },
      costSummary: null,
    });
  });

  it('无 running 会话 → idle', () => {
    useAgentStore.setState({
      sessions: [mkSession('completed'), mkSession('failed')],
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-bar')).toHaveTextContent(/idle/);
    expect(screen.getByTestId('gate-bar').querySelector('[data-running="false"]')).not.toBeNull();
  });

  it('有 running 会话 → 计数显示且 data-running=true', () => {
    useAgentStore.setState({
      sessions: [mkSession('running'), mkSession('running'), mkSession('completed')],
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-bar')).toHaveTextContent(/2 个 agent 运行中/);
    expect(screen.getByTestId('gate-bar').querySelector('[data-running="true"]')).not.toBeNull();
  });
});

describe('GateBar — 成本门控/熔断（块4，启示3）', () => {
  beforeEach(() => {
    useAgentStore.setState({ sessions: [] });
    useDashboardStore.setState({
      budget: { spent: 0, total: 0, percentage: 0 },
      costSummary: null,
    });
  });

  it('超预算 → 熔断警告 + data-over=true', () => {
    useDashboardStore.setState({
      budget: { spent: 5, total: 4, percentage: 125 },
      costSummary: mkCost(5),
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-budget').getAttribute('data-over')).toBe('true');
    expect(screen.getByTestId('gate-bar')).toHaveTextContent('超预算');
    expect(screen.getByTestId('gate-bar')).toHaveTextContent('累计 $5.00');
  });

  it('预算内 → 进度条渲染但不熔断', () => {
    useDashboardStore.setState({
      budget: { spent: 1, total: 4, percentage: 25 },
      costSummary: mkCost(1),
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-budget').getAttribute('data-over')).toBe('false');
    expect(screen.getByTestId('gate-bar')).not.toHaveTextContent('超预算');
    expect(screen.getByTestId('gate-budget-bar')).toBeInTheDocument();
  });

  it('未设预算(total=0) → 不渲染预算条，仅累计成本', () => {
    useDashboardStore.setState({
      budget: { spent: 0, total: 0, percentage: 0 },
      costSummary: mkCost(2.5),
    });
    render(<GateBar />);
    expect(screen.queryByTestId('gate-budget')).toBeNull();
    expect(screen.getByTestId('gate-bar')).toHaveTextContent('累计 $2.50');
  });
});

describe('MemoryRail — 记忆概览（块5，轴线C）', () => {
  beforeEach(() => {
    // orchestrate 视图避开 GitPanel(其调 git invoke)；记忆统计与视图无关
    useNavigationStore.setState({
      activeView: 'orchestrate',
      activeProject: null,
      selectedConversationId: null,
    });
    useAgentStore.setState({ sessions: [] });
    useKnowledgeStore.setState({ entries: [], searchResults: [] });
  });

  it('compact events → 压缩次数 + 归档消息数', () => {
    useNavigationStore.setState({ selectedConversationId: 'c1' });
    useAgentStore.setState({
      sessions: [
        mkSession('completed', {
          conversationId: 'c1',
          blocks: [
            { kind: 'compact', summary: 's1', archived_at: '2026-01-01T00:00:00Z', dropped_count: 5, is_error: false },
            { kind: 'compact', summary: 's2', archived_at: '2026-01-01T00:00:00Z', dropped_count: 3, is_error: false },
          ],
        }),
      ],
    });
    render(<MemoryRail />);
    expect(screen.getByTestId('memory-compaction-stat')).toHaveTextContent('压缩 2 次 · 归档 8 条消息');
  });

  it('无 compact events → 占位', () => {
    useNavigationStore.setState({ selectedConversationId: 'c1' });
    useAgentStore.setState({
      sessions: [
        mkSession('completed', { conversationId: 'c1', blocks: [{ kind: 'text', content: 'x' }] }),
      ],
    });
    render(<MemoryRail />);
    expect(screen.queryByTestId('memory-compaction-stat')).toBeNull();
    expect(screen.getByTestId('memory-rail')).toHaveTextContent('无压缩记录');
    // 无 activeProject → 反射占位提示选项目
    expect(screen.getByTestId('reflection-placeholder')).toHaveTextContent('选择项目后展示反射笔记');
  });

  it('react_reflection 条目 → 反射列表（仅反射类目，按 createdAt 倒序）', () => {
    // 块5b：复用既有 get_knowledge_for_project IPC + react_reflection category
    // （后端 persist_completion_memory 已写）。此处直接注入 store 绕过 invoke。
    useKnowledgeStore.setState({
      entries: [
        {
          id: 'r-new',
          projectHash: 'h',
          category: 'react_reflection',
          title: '修了 cargo 错',
          content: '...',
          sourceAgent: 'react_kernel',
          sourceSessionId: null,
          sourceType: 'session',
          confidence: 0.9,
          createdAt: '2026-07-02T10:00:00Z',
          updatedAt: '2026-07-02T10:00:00Z',
          accessCount: 0,
        },
        {
          id: 'r-old',
          projectHash: 'h',
          category: 'react_reflection',
          title: '老反思',
          content: '...',
          sourceAgent: 'react_kernel',
          sourceSessionId: null,
          sourceType: 'session',
          confidence: 0.5,
          createdAt: '2026-07-01T10:00:00Z',
          updatedAt: '2026-07-01T10:00:00Z',
          accessCount: 0,
        },
        {
          id: 'x',
          projectHash: 'h',
          category: 'react_session',
          title: '不该显示',
          content: '...',
          sourceAgent: 'react_kernel',
          sourceSessionId: null,
          sourceType: 'session',
          confidence: 0.8,
          createdAt: '2026-07-03T10:00:00Z',
          updatedAt: '2026-07-03T10:00:00Z',
          accessCount: 0,
        },
      ],
      searchResults: [],
    });
    render(<MemoryRail />);
    const list = screen.getByTestId('reflection-list');
    expect(list).toHaveTextContent('修了 cargo 错');
    expect(list).toHaveTextContent('老反思');
    expect(list).not.toHaveTextContent('不该显示'); // 仅 react_reflection，排除 react_session
    // 倒序：r-new(07-02) 排在 r-old(07-01) 之前
    const txt = list.textContent ?? '';
    expect(txt.indexOf('修了 cargo 错')).toBeLessThan(txt.indexOf('老反思'));
  });
});
