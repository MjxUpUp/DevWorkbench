import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { PlanBar } from '../PlanBar';
import { GateBar } from '../GateBar';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useAgentStore } from '../../../stores/agentStore';
import { useOrchestrateStore } from '../../../stores/orchestrateStore';
import type { Session, SessionStatus } from '../../../types';

/**
 * 起底重构 块1 骨架 + 块2 PlanBar 进度派生 契约测试。
 *
 * 4 区结构与 Stage 视图路由的正确性由 E2E 覆盖（app.spec → chat 路径，
 * orchestrate.spec → orchestrate 路径，均 7/7 绿）。此处单测三条纯派生逻辑：
 *  - PlanBar：activeView → 运行模式（轴线A 位置可见）
 *  - PlanBar：sessions/nodes → plan 进度（轴线A 执行可见，块2）
 *  - GateBar：sessions → 运行计数（门控层实时态）
 * 派生函数越纯越该单测；带副作用/渲染的集成留给 E2E。
 */

// 最小合法 Session——其余字段对所测逻辑无关，但仍按接口填全避免未来字段收紧时
// 静默漏配。第二参 partial 覆盖（块2 进度测试要 conversationId/blocks/startedAt）。
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
      sessions: [
        mkSession('running'),
        mkSession('running'),
        mkSession('completed'),
      ],
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-bar')).toHaveTextContent(/2 个 agent 运行中/);
    expect(screen.getByTestId('gate-bar').querySelector('[data-running="true"]')).not.toBeNull();
  });
});
