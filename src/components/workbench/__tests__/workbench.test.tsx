import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { PlanBar } from '../PlanBar';
import { GateBar } from '../GateBar';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useAgentStore } from '../../../stores/agentStore';
import { useDashboardStore } from '../../../stores/dashboardStore';
import type { Session, SessionStatus, CostSummary } from '../../../types';

// GateBar useEffect 调 fetchDashboard → invoke；mock 成 reject 让 fetchDashboard
// catch（不覆盖下方 setState 的 budget/costSummary），隔离测渲染派生。
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.reject(new Error('test: no backend'))),
}));

/**
 * 起底重构 块1-4 契约测试。
 *  4 区结构 + Stage 路由 + Chat 步骤分组的正确性由 E2E 覆盖。此处单测纯派生：
 *  - PlanBar：activeView → mode-segmented 高亮（轴线A 位置）+ sessions → plan 进度
 *  - GateBar：sessions → 运行计数 + budget → 成本门控/熔断（块4）
 *
 * 砍 DAG 后 PlanBar mode-segmented 只剩 Chat / Trace 两段（orchestrate 用例移除）。
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
  });

  it('task 视图 → mode-segmented 高亮 Chat，plan∈LLM context', () => {
    useNavigationStore.setState({ activeView: 'task' });
    render(<PlanBar />);
    const chatBtn = screen.getByTestId('plan-mode');
    expect(chatBtn).toHaveTextContent('Chat');
    expect(chatBtn.getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('plan-bar')).toHaveTextContent(/plan ∈ LLM context/);
  });

  it('trace 视图 → mode-segmented 高亮 Trace', () => {
    useNavigationStore.setState({ activeView: 'trace' });
    render(<PlanBar />);
    const traceBtn = screen.getByTestId('plan-mode');
    expect(traceBtn).toHaveTextContent('Trace');
    expect(traceBtn.getAttribute('aria-pressed')).toBe('true');
  });

  it('mode-segmented 点击 → 切换 activeView', () => {
    useNavigationStore.setState({ activeView: 'task' });
    render(<PlanBar />);
    // task active 时，trace 段是非 active → testid mode-seg-trace
    fireEvent.click(screen.getByTestId('mode-seg-trace'));
    expect(useNavigationStore.getState().activeView).toBe('trace');
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

  // B1 plan stepper（tool 调用序列横向渲染）已移除：与 Stage 步骤块重复，且 chat 模式
  // plan∈LLM context 隐式拿不到真 plan 阶段，tool 序列只是伪代理。进度归 planProgress 汇总。
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

  it('熔断保护徽章常驻（G3 后端隐式，前端只显就绪不伪造计数）', () => {
    // G3 step-重复熔断 + 成本熔断由后端 react_agent 隐式执行；前端无实时 trip 计数可
    // 暴露，只显「就绪」状态——触发时以 Failed 终态浮现，不伪造 0 计数。
    render(<GateBar />);
    expect(screen.getByTestId('gate-guard')).toHaveTextContent(/就绪/);
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

  it('G2: 接近预算（≥80%）→ 黄色接近警告 data-near=true，但不熔断', () => {
    // 三态：正常 / near(≥80% 黄) / over(≥100% 红)。near 前置可见，让用户在熔断前察觉。
    useDashboardStore.setState({
      budget: { spent: 3.5, total: 4, percentage: 87 },
      costSummary: mkCost(3.5),
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-budget').getAttribute('data-near')).toBe('true');
    expect(screen.getByTestId('gate-budget').getAttribute('data-over')).toBe('false');
    expect(screen.getByTestId('gate-bar')).toHaveTextContent('接近预算');
    expect(screen.getByTestId('gate-bar')).not.toHaveTextContent('超预算');
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
